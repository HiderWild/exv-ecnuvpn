# WebView Window Size and Tray Icon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Align the native WebView shell with the Electron-era main window size and keep a native entry icon visible for the whole app lifetime on Windows and macOS.

**Architecture:** Window bounds move into a shared UI-shell constant so Windows, macOS, and Linux native shells do not drift. Persistent tray/menu-bar entry remains platform-owned native shell behavior; renderer code only supplies the existing close prompt UI and replies through the host bridge. Windows uses `Shell_NotifyIconW`; macOS uses `NSStatusItem`; Linux only receives window-size alignment in this plan because the current build chain does not include StatusNotifier/AppIndicator dependencies.

**Tech Stack:** C++20, Win32/WebView2, Objective-C++ Cocoa/WKWebView, GTK/WebKitGTK, CMake, existing CTest and host test runners.

---

## Evidence and Scope

Electron-era bounds came from the removed `webui/desktop/main/index.ts` in `c23cb2f^`:

```ts
const advancedWindowBounds = {
  width: 972,
  height: 563,
}

const minimalWindowBounds = {
  width: 302,
  height: 118,
}
```

Current native WebView shell sizes:

- `src/platform/win32/ui_shell/webview2_host_win32.cpp`: `CreateWindowExW(..., 1180, 760, ...)`
- `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`: `NSMakeRect(0, 0, 1180, 760)`
- `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`: `gtk_window_set_default_size(..., 1180, 760)`

Current native WebView shell tray/status state:

- Windows has no `Shell_NotifyIconW` integration.
- macOS has no `NSStatusItem`.
- Linux has no tray/status notifier integration and no linked tray dependency.
- The old Electron tray was created only inside the close-to-tray path: `if (result.action === 'tray') { ensureTray(); mainWindow?.hide() }`.

This plan changes that product rule: Windows and macOS create their entry icon at shell startup and keep it visible until process shutdown.

---

## File Structure

- Create `src/app/ui_shell/window_layout.hpp`: shared Electron-compatible window size constants.
- Modify `tests/ui_shell_contract_test.cpp`: verifies shared advanced/minimal bounds.
- Modify `src/platform/win32/ui_shell/webview2_host_win32.hpp`: exposes testable Win32 default bounds and tray menu model.
- Modify `src/platform/win32/ui_shell/webview2_host_win32.cpp`: consumes shared bounds, creates persistent tray icon, handles tray commands, wires close prompt results.
- Modify `tests/win32_webview2_runtime_test.cpp`: verifies bounds, renderer URI behavior, and tray menu model.
- Modify `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`: consumes shared bounds, creates persistent `NSStatusItem`, wires close prompt results.
- Modify `tests/darwin_wkwebview_runtime_test.cpp`: verifies bounds and status menu model.
- Modify `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`: consumes shared bounds and disables resizing to match Electron main window behavior.
- Modify `tests/linux_webkitgtk_runtime_test.cpp`: verifies Linux default bounds.
- Modify `CMakeLists.txt`: add `shell32` where needed for Win32 tray APIs.

---

### Task 1: Add Shared Electron Window Bounds

**Files:**
- Create: `src/app/ui_shell/window_layout.hpp`
- Modify: `tests/ui_shell_contract_test.cpp`

- [ ] **Step 1: Write the failing shared bounds test**

Add this include near the existing UI shell includes in `tests/ui_shell_contract_test.cpp`:

```cpp
#include "app/ui_shell/window_layout.hpp"
```

Add this check near the start of `main()` after `using namespace ecnuvpn::ui_shell;`:

```cpp
  if (kElectronAdvancedWindowBounds.width != 972 ||
      kElectronAdvancedWindowBounds.height != 563) {
    return 1;
  }
  if (kElectronMinimalWindowBounds.width != 302 ||
      kElectronMinimalWindowBounds.height != 118) {
    return 1;
  }
```

- [ ] **Step 2: Run the failing test**

Run:

```powershell
cmake --build build-windows\cpp --target ui_shell_contract_test
build-windows\cpp\ui_shell_contract_test.exe
```

Expected: build fails because `app/ui_shell/window_layout.hpp` does not exist.

- [ ] **Step 3: Implement the shared bounds header**

Create `src/app/ui_shell/window_layout.hpp`:

```cpp
#pragma once

namespace ecnuvpn::ui_shell {

struct WindowBounds {
  int width;
  int height;
};

inline constexpr WindowBounds kElectronAdvancedWindowBounds{972, 563};
inline constexpr WindowBounds kElectronMinimalWindowBounds{302, 118};

} // namespace ecnuvpn::ui_shell
```

- [ ] **Step 4: Verify the test passes**

Run:

```powershell
cmake --build build-windows\cpp --target ui_shell_contract_test
build-windows\cpp\ui_shell_contract_test.exe
```

Expected: exit code `0`.

- [ ] **Step 5: Commit Task 1**

Run:

```powershell
git add src/app/ui_shell/window_layout.hpp tests/ui_shell_contract_test.cpp
git commit -m "ui-shell: define Electron-compatible window bounds"
```

---

### Task 2: Apply Electron Advanced Bounds to Native WebView Windows

**Files:**
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.hpp`
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- Modify: `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
- Modify: `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`
- Modify: `tests/win32_webview2_runtime_test.cpp`
- Modify: `tests/darwin_wkwebview_runtime_test.cpp`
- Modify: `tests/linux_webkitgtk_runtime_test.cpp`

- [ ] **Step 1: Write failing Win32 bounds test**

Add this assertion to `tests/win32_webview2_runtime_test.cpp` after `auto window = create_webview2_window();`:

```cpp
  const ecnuvpn::ui_shell::WindowBounds win32_bounds =
      webview2_default_window_bounds();
  if (win32_bounds.width != 972 || win32_bounds.height != 563) {
    return 1;
  }
```

Add this include:

```cpp
#include "app/ui_shell/window_layout.hpp"
```

Add this declaration to `src/platform/win32/ui_shell/webview2_host_win32.hpp`:

```cpp
ecnuvpn::ui_shell::WindowBounds webview2_default_window_bounds();
```

- [ ] **Step 2: Verify Win32 test fails**

Run:

```powershell
cmake --build build-windows\cpp --target win32_webview2_runtime_test
```

Expected: link or compile failure because `webview2_default_window_bounds()` is declared but not defined.

- [ ] **Step 3: Implement Win32 bounds**

In `src/platform/win32/ui_shell/webview2_host_win32.cpp`, add:

```cpp
#include "app/ui_shell/window_layout.hpp"
```

Add this public function near the existing `webview2_renderer_uri()` function:

```cpp
ecnuvpn::ui_shell::WindowBounds webview2_default_window_bounds() {
  return ecnuvpn::ui_shell::kElectronAdvancedWindowBounds;
}
```

Change `create_window()` to use Electron-era bounds and non-resizable main-window style:

```cpp
    const auto bounds = webview2_default_window_bounds();
    constexpr DWORD kMainWindowStyle =
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    hwnd_ = CreateWindowExW(0, window_class.lpszClassName, L"ECNU VPN",
                            kMainWindowStyle, CW_USEDEFAULT, CW_USEDEFAULT,
                            bounds.width, bounds.height, nullptr, nullptr,
                            instance_, this);
```

- [ ] **Step 4: Write macOS and Linux failing bounds tests**

In `tests/darwin_wkwebview_runtime_test.cpp`, declare:

```cpp
ecnuvpn::ui_shell::WindowBounds wkwebview_default_window_bounds();
```

Add:

```cpp
  const auto darwin_bounds =
      ecnuvpn::platform::darwin::ui_shell::wkwebview_default_window_bounds();
  if (darwin_bounds.width != 972 || darwin_bounds.height != 563) {
    return 1;
  }
```

In `tests/linux_webkitgtk_runtime_test.cpp`, declare:

```cpp
ecnuvpn::ui_shell::WindowBounds webkitgtk_default_window_bounds();
```

Add:

```cpp
  const auto linux_bounds =
      ecnuvpn::platform::linux_ui_shell::webkitgtk_default_window_bounds();
  if (linux_bounds.width != 972 || linux_bounds.height != 563) {
    return 7;
  }
```

Add `#include "app/ui_shell/window_layout.hpp"` to both test files.

- [ ] **Step 5: Implement macOS bounds**

In `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`, add:

```cpp
#include "app/ui_shell/window_layout.hpp"
```

Add this function outside the anonymous namespace:

```cpp
ecnuvpn::ui_shell::WindowBounds wkwebview_default_window_bounds() {
  return ecnuvpn::ui_shell::kElectronAdvancedWindowBounds;
}
```

Update `create_window()`:

```objc
    const auto bounds = wkwebview_default_window_bounds();
    const NSRect frame = NSMakeRect(0, 0, bounds.width, bounds.height);
    window_ = [[NSWindow alloc]
        initWithContentRect:frame
                  styleMask:(NSWindowStyleMaskTitled |
                             NSWindowStyleMaskClosable |
                             NSWindowStyleMaskMiniaturizable)
                    backing:NSBackingStoreBuffered
                      defer:NO];
```

After setting the title:

```objc
    [window_ setMinSize:NSMakeSize(bounds.width, bounds.height)];
    [window_ setMaxSize:NSMakeSize(bounds.width, bounds.height)];
```

- [ ] **Step 6: Implement Linux bounds**

In `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`, add:

```cpp
#include "app/ui_shell/window_layout.hpp"
```

Add this function outside the anonymous namespace:

```cpp
ecnuvpn::ui_shell::WindowBounds webkitgtk_default_window_bounds() {
  return ecnuvpn::ui_shell::kElectronAdvancedWindowBounds;
}
```

Update `create_window()`:

```cpp
    const auto bounds = webkitgtk_default_window_bounds();
    gtk_window_set_default_size(GTK_WINDOW(window_), bounds.width,
                                bounds.height);
    gtk_window_set_resizable(GTK_WINDOW(window_), FALSE);
```

- [ ] **Step 7: Verify platform tests**

On Windows:

```powershell
cmake --build build-windows\cpp --target win32_webview2_runtime_test ui_shell_contract_test
build-windows\cpp\win32_webview2_runtime_test.exe
build-windows\cpp\ui_shell_contract_test.exe
```

On macOS:

```bash
cmake --build build/macos/cpp --target darwin_wkwebview_runtime_test ui_shell_contract_test
build/macos/cpp/darwin_wkwebview_runtime_test
build/macos/cpp/ui_shell_contract_test
```

On Linux:

```bash
cmake --build build/linux/cpp --target linux_webkitgtk_runtime_test ui_shell_contract_test
build/linux/cpp/linux_webkitgtk_runtime_test
build/linux/cpp/ui_shell_contract_test
```

Expected: all commands exit `0`.

- [ ] **Step 8: Commit Task 2**

Run:

```powershell
git add src/platform/win32/ui_shell/webview2_host_win32.hpp src/platform/win32/ui_shell/webview2_host_win32.cpp src/platform/darwin/ui_shell/wk_webview_host_darwin.mm src/platform/linux/ui_shell/webkitgtk_host_linux.cpp tests/win32_webview2_runtime_test.cpp tests/darwin_wkwebview_runtime_test.cpp tests/linux_webkitgtk_runtime_test.cpp
git commit -m "ui-shell: align native WebView window size with Electron"
```

---

### Task 3: Create a Persistent Windows Tray Icon

**Files:**
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.hpp`
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- Modify: `tests/win32_webview2_runtime_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Write failing Windows tray menu model test**

Add these declarations to `src/platform/win32/ui_shell/webview2_host_win32.hpp`:

```cpp
struct WebView2TrayMenuItem {
  std::wstring label;
  int command_id;
  bool separator;
};

std::vector<WebView2TrayMenuItem> webview2_tray_menu_model();
bool webview2_should_create_tray_on_start();
```

Add `#include <vector>` to the header.

Add this check to `tests/win32_webview2_runtime_test.cpp`:

```cpp
  if (!webview2_should_create_tray_on_start()) {
    return 1;
  }
  const auto tray_menu = webview2_tray_menu_model();
  if (tray_menu.size() != 3 || tray_menu[0].label != L"显示 ECNU VPN" ||
      !tray_menu[1].separator || tray_menu[2].label != L"退出") {
    return 1;
  }
```

- [ ] **Step 2: Verify tray test fails**

Run:

```powershell
cmake --build build-windows\cpp --target win32_webview2_runtime_test
```

Expected: link failure for `webview2_tray_menu_model()` and `webview2_should_create_tray_on_start()`.

- [ ] **Step 3: Add shell32 linkage**

In `CMakeLists.txt`, update Win32 links:

```cmake
target_link_libraries(win32_webview2_runtime_test PRIVATE advapi32 urlmon shell32)
```

and:

```cmake
target_link_libraries(exv-ui PRIVATE advapi32 urlmon shell32 exv-core)
```

- [ ] **Step 4: Implement Windows tray model**

In `src/platform/win32/ui_shell/webview2_host_win32.cpp`, add:

```cpp
constexpr int kTrayCommandShow = 1001;
constexpr int kTrayCommandQuit = 1002;
constexpr UINT kTrayCallbackMessage = WM_APP + 0x42;
constexpr UINT kTrayIconId = 1;
```

Add:

```cpp
bool webview2_should_create_tray_on_start() {
  return true;
}

std::vector<WebView2TrayMenuItem> webview2_tray_menu_model() {
  return {
      {L"显示 ECNU VPN", kTrayCommandShow, false},
      {L"", 0, true},
      {L"退出", kTrayCommandQuit, false},
  };
}
```

- [ ] **Step 5: Implement Windows tray lifecycle**

Add `#include <shellapi.h>`.

Add a `NOTIFYICONDATAW tray_icon_{};` member and `bool tray_icon_added_ = false;`.

Add methods inside `WebView2Window`:

```cpp
  bool create_tray_icon() {
    if (!webview2_should_create_tray_on_start() || !hwnd_ || tray_icon_added_) {
      return true;
    }
    tray_icon_.cbSize = sizeof(tray_icon_);
    tray_icon_.hWnd = hwnd_;
    tray_icon_.uID = kTrayIconId;
    tray_icon_.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    tray_icon_.uCallbackMessage = kTrayCallbackMessage;
    tray_icon_.hIcon = LoadIconW(nullptr, IDI_APPLICATION);
    wcscpy_s(tray_icon_.szTip, L"ECNU VPN");
    tray_icon_added_ = Shell_NotifyIconW(NIM_ADD, &tray_icon_) == TRUE;
    return tray_icon_added_;
  }

  void destroy_tray_icon() {
    if (!tray_icon_added_) {
      return;
    }
    Shell_NotifyIconW(NIM_DELETE, &tray_icon_);
    tray_icon_added_ = false;
  }
```

Call `create_tray_icon()` after `create_window()` succeeds and before `ShowWindow(hwnd_, SW_SHOW)`.

Call `destroy_tray_icon()` in the destructor and before returning from `run()`.

- [ ] **Step 6: Implement Windows tray menu commands**

Add:

```cpp
  void show_from_tray() {
    if (!hwnd_) {
      return;
    }
    ShowWindow(hwnd_, SW_SHOW);
    SetForegroundWindow(hwnd_);
  }

  void quit_from_tray() {
    force_quit_ = true;
    running_ = false;
    if (hwnd_) {
      DestroyWindow(hwnd_);
    }
  }

  void show_tray_menu() {
    HMENU menu = CreatePopupMenu();
    if (!menu) {
      return;
    }
    for (const auto &item : webview2_tray_menu_model()) {
      if (item.separator) {
        AppendMenuW(menu, MF_SEPARATOR, 0, nullptr);
      } else if (!item.label.empty()) {
        AppendMenuW(menu, MF_STRING, static_cast<UINT_PTR>(item.command_id),
                    item.label.c_str());
      }
    }
    POINT cursor{};
    GetCursorPos(&cursor);
    SetForegroundWindow(hwnd_);
    const UINT command = TrackPopupMenu(
        menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, cursor.x, cursor.y, 0, hwnd_,
        nullptr);
    DestroyMenu(menu);
    if (command == kTrayCommandShow) {
      show_from_tray();
    } else if (command == kTrayCommandQuit) {
      quit_from_tray();
    }
  }
```

Add `bool force_quit_ = false;` as a member.

In `window_proc`, handle:

```cpp
      case kTrayCallbackMessage:
        if (lparam == WM_LBUTTONUP) {
          self->show_from_tray();
        } else if (lparam == WM_RBUTTONUP || lparam == WM_CONTEXTMENU) {
          self->show_tray_menu();
        }
        return 0;
```

- [ ] **Step 7: Verify Windows tray tests and build**

Run:

```powershell
cmake --build build-windows\cpp --target win32_webview2_runtime_test exv-ui
build-windows\cpp\win32_webview2_runtime_test.exe
```

Expected: build succeeds and test exits `0`.

- [ ] **Step 8: Commit Task 3**

Run:

```powershell
git add CMakeLists.txt src/platform/win32/ui_shell/webview2_host_win32.hpp src/platform/win32/ui_shell/webview2_host_win32.cpp tests/win32_webview2_runtime_test.cpp
git commit -m "ui-shell: keep Windows tray icon visible"
```

---

### Task 4: Create a Persistent macOS Menu-Bar Status Item

**Files:**
- Modify: `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
- Modify: `tests/darwin_wkwebview_runtime_test.cpp`

- [ ] **Step 1: Write failing macOS status menu model test**

Add this struct and function declaration to the test namespace in `tests/darwin_wkwebview_runtime_test.cpp`:

```cpp
struct WkWebViewStatusMenuItem {
  std::string label;
  int command_id;
  bool separator;
};
std::vector<WkWebViewStatusMenuItem> wkwebview_status_menu_model();
bool wkwebview_should_create_status_item_on_start();
```

Add `#include <vector>`.

Add:

```cpp
  if (!ecnuvpn::platform::darwin::ui_shell::
          wkwebview_should_create_status_item_on_start()) {
    return 1;
  }
  const auto status_menu =
      ecnuvpn::platform::darwin::ui_shell::wkwebview_status_menu_model();
  if (status_menu.size() != 3 || status_menu[0].label != "显示 ECNU VPN" ||
      !status_menu[1].separator || status_menu[2].label != "退出") {
    return 1;
  }
```

- [ ] **Step 2: Verify macOS status test fails**

Run on macOS:

```bash
cmake --build build/macos/cpp --target darwin_wkwebview_runtime_test
```

Expected: link failure for the new status item helper functions.

- [ ] **Step 3: Implement macOS status model**

In `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`, add:

```cpp
struct WkWebViewStatusMenuItem {
  std::string label;
  int command_id;
  bool separator;
};

constexpr int kStatusCommandShow = 2001;
constexpr int kStatusCommandQuit = 2002;

bool wkwebview_should_create_status_item_on_start() {
  return true;
}

std::vector<WkWebViewStatusMenuItem> wkwebview_status_menu_model() {
  return {
      {"显示 ECNU VPN", kStatusCommandShow, false},
      {"", 0, true},
      {"退出", kStatusCommandQuit, false},
  };
}
```

- [ ] **Step 4: Add Objective-C status item target**

Add an Objective-C target wrapper before the `WkWebViewWindow` class:

```objc
@interface EcnuVpnStatusItemTarget : NSObject {
  ecnuvpn::platform::darwin::ui_shell::WkWebViewWindow *owner_;
}
- (instancetype)initWithOwner:
    (ecnuvpn::platform::darwin::ui_shell::WkWebViewWindow *)owner;
- (void)showWindow:(id)sender;
- (void)quitApp:(id)sender;
@end
```

Implement it after the existing Objective-C implementations:

```objc
@implementation EcnuVpnStatusItemTarget
- (instancetype)initWithOwner:
    (ecnuvpn::platform::darwin::ui_shell::WkWebViewWindow *)owner {
  self = [super init];
  if (self != nil) {
    owner_ = owner;
  }
  return self;
}
- (void)showWindow:(id)sender {
  (void)sender;
  if (owner_ != nullptr) owner_->show_from_status_item();
}
- (void)quitApp:(id)sender {
  (void)sender;
  if (owner_ != nullptr) owner_->quit_from_status_item();
}
@end
```

- [ ] **Step 5: Implement macOS status item lifecycle**

Add members:

```objc
  NSStatusItem *status_item_ = nil;
  NSMenu *status_menu_ = nil;
  EcnuVpnStatusItemTarget *status_target_ = nil;
  bool force_quit_ = false;
```

Add methods inside `WkWebViewWindow`:

```objc
  void show_from_status_item() {
    if (window_ == nil) return;
    [window_ makeKeyAndOrderFront:nil];
    [[NSApplication sharedApplication] activateIgnoringOtherApps:YES];
  }

  void quit_from_status_item() {
    force_quit_ = true;
    running_ = false;
    if (window_ != nil) {
      [window_ close];
    }
  }

  NSImage *status_icon() {
    NSImage *image = [[[NSImage alloc] initWithSize:NSMakeSize(18, 18)] autorelease];
    [image lockFocus];
    [[NSColor labelColor] setFill];
    NSBezierPath *circle =
        [NSBezierPath bezierPathWithOvalInRect:NSMakeRect(3, 3, 12, 12)];
    [circle fill];
    [image unlockFocus];
    [image setTemplate:YES];
    return image;
  }

  void create_status_item() {
    if (!wkwebview_should_create_status_item_on_start() || status_item_ != nil) {
      return;
    }
    status_target_ = [[EcnuVpnStatusItemTarget alloc] initWithOwner:this];
    status_item_ = [[[NSStatusBar systemStatusBar]
        statusItemWithLength:NSSquareStatusItemLength] retain];
    [[status_item_ button] setImage:status_icon()];
    [[status_item_ button] setToolTip:@"ECNU VPN"];

    status_menu_ = [[NSMenu alloc] initWithTitle:@"ECNU VPN"];
    [status_menu_ addItemWithTitle:@"显示 ECNU VPN"
                             action:@selector(showWindow:)
                      keyEquivalent:@""];
    [[status_menu_ itemAtIndex:0] setTarget:status_target_];
    [status_menu_ addItem:[NSMenuItem separatorItem]];
    [status_menu_ addItemWithTitle:@"退出"
                             action:@selector(quitApp:)
                      keyEquivalent:@""];
    [[status_menu_ itemAtIndex:2] setTarget:status_target_];
    [status_item_ setMenu:status_menu_];
  }

  void destroy_status_item() {
    if (status_item_ != nil) {
      [[NSStatusBar systemStatusBar] removeStatusItem:status_item_];
      [status_item_ release];
      status_item_ = nil;
    }
    [status_menu_ release];
    status_menu_ = nil;
    [status_target_ release];
    status_target_ = nil;
  }
```

Call `create_status_item()` in `run()` after `[app setActivationPolicy:NSApplicationActivationPolicyRegular];`.

Call `destroy_status_item()` in `cleanup()`.

- [ ] **Step 6: Verify macOS status tests and build**

Run on macOS:

```bash
cmake --build build/macos/cpp --target darwin_wkwebview_runtime_test exv-ui
build/macos/cpp/darwin_wkwebview_runtime_test
```

Expected: build succeeds and test exits `0`.

- [ ] **Step 7: Commit Task 4**

Run:

```bash
git add src/platform/darwin/ui_shell/wk_webview_host_darwin.mm tests/darwin_wkwebview_runtime_test.cpp
git commit -m "ui-shell: keep macOS status item visible"
```

---

### Task 5: Wire Close-to-Background Behavior Without Gating Icon Creation

**Files:**
- Modify: `src/app/ui_shell/host_bridge.cpp`
- Modify: `tests/ui_shell_contract_test.cpp`
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- Modify: `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
- Modify: `tests/win32_webview2_runtime_test.cpp`
- Modify: `tests/darwin_wkwebview_runtime_test.cpp`

- [ ] **Step 1: Write failing host action tests**

In `tests/ui_shell_contract_test.cpp`, add:

```cpp
  if (!is_allowed_host_action("window.resolveClosePrompt")) {
    return 1;
  }
  if (!is_allowed_host_action("window.setMode")) {
    return 1;
  }
```

- [ ] **Step 2: Verify host action test fails**

Run:

```powershell
cmake --build build-windows\cpp --target ui_shell_contract_test
build-windows\cpp\ui_shell_contract_test.exe
```

Expected: executable returns non-zero because those actions are not allowed yet.

- [ ] **Step 3: Allow native shell window actions**

In `src/app/ui_shell/host_bridge.cpp`, update `is_allowed_host_action()`:

```cpp
  if (action == "window.resolveClosePrompt" || action == "window.setMode") {
    return true;
  }
```

- [ ] **Step 4: Make renderer bridge send close prompt replies to native host**

In both platform bridge scripts, replace:

```js
window: {
  setMode: () => Promise.resolve(),
  resolveClosePrompt: () => Promise.resolve(),
},
```

with:

```js
window: {
  setMode: (mode) => rpc('window.setMode', { mode }),
  resolveClosePrompt: (result) => rpc('window.resolveClosePrompt', { result }),
},
```

- [ ] **Step 5: Implement Win32 close prompt state**

Add members:

```cpp
  bool close_prompt_pending_ = false;
```

Change `WM_CLOSE` handling:

```cpp
      case WM_CLOSE:
        if (self->force_quit_) {
          DestroyWindow(hwnd);
        } else {
          self->request_close_decision();
        }
        return 0;
```

Add:

```cpp
  void request_close_decision() {
    if (close_prompt_pending_) {
      show_from_tray();
      return;
    }
    close_prompt_pending_ = true;
    emit_event(R"({"type":"close-request","data":{}})");
  }
```

In `on_web_message`, before forwarding to `handler_`, parse `window.setMode` and `window.resolveClosePrompt`.

For `window.setMode`, return a successful response and resize to the Electron-era mode bounds:

```cpp
  void apply_window_mode(const std::string &mode) {
    const auto bounds = mode == "minimal"
                            ? ecnuvpn::ui_shell::kElectronMinimalWindowBounds
                            : ecnuvpn::ui_shell::kElectronAdvancedWindowBounds;
    if (hwnd_) {
      SetWindowPos(hwnd_, nullptr, 0, 0, bounds.width, bounds.height,
                   SWP_NOMOVE | SWP_NOZORDER);
      resize_webview();
    }
  }
```

For `window.resolveClosePrompt`, if the payload action is `"tray"`, clear `close_prompt_pending_` and call `ShowWindow(hwnd_, SW_HIDE)`; if action is `"quit"`, clear the flag and call `quit_from_tray()`; if action is `"cancel"`, clear the flag and call `show_from_tray()`.

- [ ] **Step 6: Implement macOS close prompt state**

Add members:

```objc
  bool close_prompt_pending_ = false;
```

Change `close_from_window()`:

```objc
  void close_from_window() {
    if (force_quit_) {
      running_ = false;
      exit_code_ = 0;
      return;
    }
    request_close_decision();
  }
```

Add:

```objc
  void request_close_decision() {
    if (close_prompt_pending_) {
      show_from_status_item();
      return;
    }
    close_prompt_pending_ = true;
    emit_event(R"({"type":"close-request","data":{}})");
  }
```

In `handle_script_message`, intercept `window.setMode` and `window.resolveClosePrompt`.

For `window.setMode`, return a successful response and resize to the Electron-era mode bounds:

```objc
  void apply_window_mode(const std::string &mode) {
    const auto bounds = mode == "minimal"
                            ? ecnuvpn::ui_shell::kElectronMinimalWindowBounds
                            : ecnuvpn::ui_shell::kElectronAdvancedWindowBounds;
    if (window_ == nil) return;
    NSRect frame = [window_ frame];
    frame.size = NSMakeSize(bounds.width, bounds.height);
    [window_ setMinSize:NSMakeSize(bounds.width, bounds.height)];
    [window_ setMaxSize:NSMakeSize(bounds.width, bounds.height)];
    [window_ setFrame:frame display:YES animate:NO];
  }
```

For `window.resolveClosePrompt`, `"tray"` calls `[window_ orderOut:nil]`, `"quit"` calls `quit_from_status_item()`, `"cancel"` calls `show_from_status_item()`, and every branch clears `close_prompt_pending_`.

- [ ] **Step 7: Verify close action tests**

Run on Windows:

```powershell
cmake --build build-windows\cpp --target ui_shell_contract_test win32_webview2_runtime_test
build-windows\cpp\ui_shell_contract_test.exe
build-windows\cpp\win32_webview2_runtime_test.exe
```

Run on macOS:

```bash
cmake --build build/macos/cpp --target ui_shell_contract_test darwin_wkwebview_runtime_test
build/macos/cpp/ui_shell_contract_test
build/macos/cpp/darwin_wkwebview_runtime_test
```

Expected: all commands exit `0`.

- [ ] **Step 8: Commit Task 5**

Run:

```powershell
git add src/app/ui_shell/host_bridge.cpp tests/ui_shell_contract_test.cpp src/platform/win32/ui_shell/webview2_host_win32.cpp src/platform/darwin/ui_shell/wk_webview_host_darwin.mm tests/win32_webview2_runtime_test.cpp tests/darwin_wkwebview_runtime_test.cpp
git commit -m "ui-shell: decouple close-to-background from tray visibility"
```

---

### Task 6: End-to-End Verification

**Files:**
- No source files required.

- [ ] **Step 1: Run Windows desktop build**

Run:

```powershell
powershell.exe -NoProfile -NoLogo -ExecutionPolicy Bypass -File scripts\build-windows.ps1 desktop
```

Expected:

- `win32_webview2_runtime_test` passes.
- `ui_shell_contract_test` passes.
- native WebView package is created under `build\windows\webview\package\ECNU VPN`.

- [ ] **Step 2: Verify Windows package launch targets**

Run:

```powershell
python scripts\package_ui_shell.py --verify-launch-targets-only --package-dir "build\windows\webview\package\ECNU VPN"
```

Expected: prints `verified native WebView shell package`.

- [ ] **Step 3: Run Windows renderer smoke via CDP**

Launch packaged `exv-ui.exe` with:

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9337'
Start-Process -FilePath "build\windows\webview\package\ECNU VPN\exv-ui.exe" -ArgumentList @('--exv','bin/exv.exe','--renderer-index','webui/index.html') -WorkingDirectory "build\windows\webview\package\ECNU VPN"
```

Then inspect via CDP and verify:

- `location.href` starts with `https://appassets.ecnu-vpn.invalid/index.html`.
- `document.getElementById('app').children.length > 0`.
- `window.innerWidth` is close to the Electron-era content width for the `972x563` native window.
- Windows tray icon is visible immediately after launch, before pressing the window close button.

- [ ] **Step 4: Run macOS desktop build**

Run on macOS:

```bash
./scripts/build-macos.sh desktop
```

Expected:

- `darwin_wkwebview_runtime_test` passes.
- native WebView package is created under `build/macos/webview/package/ECNU VPN`.
- menu-bar status item appears immediately after launch.

- [ ] **Step 5: Run Linux size regression**

Run on Linux:

```bash
./scripts/build-linux.sh desktop
```

Expected:

- `linux_webkitgtk_runtime_test` passes.
- Linux native WebView window opens at `972x563` and is not resizable.
- No Linux tray/status dependency is added.

- [ ] **Step 6: Run host tests**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts host/__tests__/desktop-contract-generated.test.ts
```

Expected: all listed host tests pass.

- [ ] **Step 7: Check diff hygiene**

Run:

```powershell
git diff --check
git status --short
```

Expected: no whitespace errors. Only task-related files are modified or committed; unrelated dirty files remain unstaged.

- [ ] **Step 8: Commit verification notes if scripts or tests changed during verification**

If no files changed during verification, do not create an empty commit.
