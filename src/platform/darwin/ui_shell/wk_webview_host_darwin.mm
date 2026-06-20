#include "app/ui_shell/close_preference.hpp"
#include "app/ui_shell/host_bridge.hpp"
#include "app/ui_shell/ui_window.hpp"
#include "app/ui_shell/window_layout.hpp"

#if defined(EXV_BUILD_UI_SHELL)
#import <Cocoa/Cocoa.h>
#import <WebKit/WebKit.h>
#endif

#include <cstdint>
#include <filesystem>
#include <memory>
#include <nlohmann/json.hpp>
#include <string>
#include <utility>
#include <vector>

namespace exv::platform::darwin::ui_shell {
class WkWebViewWindow;
}

#if defined(EXV_BUILD_UI_SHELL)
@interface ExvScriptMessageHandler : NSObject <WKScriptMessageHandler> {
  exv::platform::darwin::ui_shell::WkWebViewWindow *owner_;
}
- (instancetype)initWithOwner:
    (exv::platform::darwin::ui_shell::WkWebViewWindow *)owner;
@end

@interface ExvWindowDelegate : NSObject <NSWindowDelegate> {
  exv::platform::darwin::ui_shell::WkWebViewWindow *owner_;
}
- (instancetype)initWithOwner:
    (exv::platform::darwin::ui_shell::WkWebViewWindow *)owner;
@end

@interface ExvNavigationDelegate : NSObject <WKNavigationDelegate> {
  exv::platform::darwin::ui_shell::WkWebViewWindow *owner_;
}
- (instancetype)initWithOwner:
    (exv::platform::darwin::ui_shell::WkWebViewWindow *)owner;
@end

@interface ExvUIDelegate : NSObject <WKUIDelegate>
@end

@interface ExvStatusItemTarget : NSObject {
  exv::platform::darwin::ui_shell::WkWebViewWindow *owner_;
}
- (instancetype)initWithOwner:
    (exv::platform::darwin::ui_shell::WkWebViewWindow *)owner;
- (void)showWindow:(id)sender;
- (void)quitApp:(id)sender;
@end
#endif

namespace exv::platform::darwin::ui_shell {
namespace {

#if defined(EXV_BUILD_UI_SHELL)
NSString *ns_string(const std::string &value) {
  return [[[NSString alloc] initWithBytes:value.data()
                                   length:value.size()
                                 encoding:NSUTF8StringEncoding] autorelease];
}

std::string utf8_from_ns_string(NSString *value) {
  if (value == nil) {
    return {};
  }
  const char *utf8 = [value UTF8String];
  return utf8 ? std::string(utf8) : std::string();
}

bool is_supported_external_url(std::string_view url) {
  return url.starts_with("https://") || url.starts_with("http://");
}

NSURL *renderer_url(const exv::ui_shell::RendererAssets &renderer) {
  if (renderer.kind == exv::ui_shell::RendererAssetKind::DevServer) {
    return [NSURL URLWithString:ns_string(renderer.location)];
  }

  const std::filesystem::path absolute =
      std::filesystem::absolute(std::filesystem::path(renderer.location));
  return [NSURL fileURLWithPath:ns_string(absolute.string())];
}

NSURL *renderer_read_access_url(
    const exv::ui_shell::RendererAssets &renderer) {
  if (renderer.kind == exv::ui_shell::RendererAssetKind::DevServer) {
    return nil;
  }

  const std::filesystem::path absolute =
      std::filesystem::absolute(std::filesystem::path(renderer.location));
  return [NSURL fileURLWithPath:ns_string(absolute.parent_path().string())
                    isDirectory:YES];
}

NSString *bridge_script() {
  static constexpr char kBridgeScript[] = R"JS(
(() => {
  if (window.exv || !window.webkit || !window.webkit.messageHandlers || !window.webkit.messageHandlers.exvHost) return;
  let nextId = 1;
  const pending = new Map();
  const subscribers = new Set();
  function rpc(action, payload) {
    const id = nextId++;
    const request = { id, action, payload: payload ?? {} };
    window.webkit.messageHandlers.exvHost.postMessage(JSON.stringify(request));
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
    });
  }
  function unsupported(name) {
    return Promise.reject(new Error(`${name} is not available in the native WebView shell yet.`));
  }
  window.__exvHostReceive = (message) => {
    if (message && typeof message.id === 'number' && pending.has(message.id)) {
      const callbacks = pending.get(message.id);
      pending.delete(message.id);
      if (message.ok) callbacks.resolve(message.data);
      else {
        const error = new Error(message.message || 'Desktop RPC failed');
        error.code = message.code;
        callbacks.reject(error);
      }
      return;
    }
    subscribers.forEach((handler) => handler(message));
  };
  window.exv = {
    status: { get: () => rpc('status.get') },
    vpn: {
      connect: (password) => rpc('vpn.connect', { password }),
      disconnect: () => rpc('vpn.disconnect'),
      connectElevated: (password) => rpc('vpn.connect', { password, allow_direct_fallback: true }),
      disconnectElevated: (backend) => rpc('vpn.disconnect', { backend, allow_direct_fallback: true }),
    },
    config: {
      getAuth: () => rpc('config.getAuth'),
      saveAuth: (input) => rpc('config.saveAuth', input),
      getSettings: () => rpc('config.getSettings'),
      saveSettings: (input) => rpc('config.saveSettings', input),
      getKey: () => rpc('config.getKey'),
      importConfig: (input) => rpc('config.import', input ?? {}),
      exportConfig: (input) => rpc('config.export', input ?? {}),
      reset: (confirm) => rpc('config.reset', { confirm }),
    },
    routes: {
      list: () => rpc('routes.list'),
      add: (cidr) => rpc('routes.add', { cidr }),
      remove: (cidr) => rpc('routes.remove', { cidr }),
      reset: () => rpc('routes.reset'),
    },
    service: {
      status: () => rpc('service.status'),
      install: () => rpc('service.install'),
      uninstall: () => rpc('service.uninstall'),
    },
    cli: {
      status: () => rpc('cli.status'),
      install: () => rpc('cli.install'),
      uninstall: () => rpc('cli.uninstall'),
    },
    logs: { list: (options) => rpc('logs.list', options ?? {}) },
    runtime: { status: () => rpc('runtime.status') },
    drivers: {
      status: () => rpc('drivers.status'),
      install: (driver) => rpc('drivers.install', { driver }),
    },
    key: {
      status: () => rpc('key.status'),
      reset: (confirm) => rpc('key.reset', { confirm }),
    },
    maintenance: {
      inspectCore: () => rpc('maintenance.inspectCore'),
      killStaleCore: (confirm) => rpc('maintenance.killStaleCore', { confirm }),
    },
    window: {
      setMode: (mode, request) => rpc('window.resizeForMode', { mode, request }),
      resizeForMode: (mode, request) => rpc('window.resizeForMode', { mode, request }),
      minimize: () => rpc('window.minimize'),
      requestClose: () => rpc('window.requestClose'),
      resolveClosePrompt: (result) => rpc('window.resolveClosePrompt', { result }),
      startDrag: (drag) => rpc('window.startDrag', drag ?? {}),
    },
    shell: {
      openExternal: (url) => rpc('shell.openExternal', { url }),
    },
    modal: {
      serviceInstallPrompt: () => Promise.resolve('dismiss'),
      passwordPrompt: (message) => Promise.resolve(window.prompt(message) ?? null),
      confirmPrompt: (message) => Promise.resolve(window.confirm(message)),
      getPayload: () => Promise.resolve(null),
      resolve: () => Promise.resolve(),
    },
    events: {
      subscribe: (handler) => {
        subscribers.add(handler);
        return () => subscribers.delete(handler);
      },
    },
    core: {
      restart: () => unsupported('core.restart'),
      quit: () => { window.close(); return Promise.resolve(); },
    },
  };
})();
)JS";
  return ns_string(kBridgeScript);
}
#endif

} // namespace

[[nodiscard]] exv::ui_shell::WindowBounds
wkwebview_default_window_bounds() noexcept;

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
      {"显示 EXV", kStatusCommandShow, false},
      {"", 0, true},
      {"退出", kStatusCommandQuit, false},
  };
}

#if defined(EXV_BUILD_UI_SHELL)
class WkWebViewWindow final : public exv::ui_shell::UiWindow {
public:
  ~WkWebViewWindow() override { cleanup(); }

  void set_message_handler(exv::ui_shell::HostMessageHandler handler) override {
    handler_ = std::move(handler);
  }

  int run(const exv::ui_shell::UiWindowConfig &config) override {
    if (![NSThread isMainThread]) {
      return 70;
    }

    @autoreleasepool {
      active_config_ = config;
      exit_code_ = 0;
      renderer_ready_ = false;

      NSApplication *app = [NSApplication sharedApplication];
      [app setActivationPolicy:NSApplicationActivationPolicyRegular];
      create_status_item();

      if (!create_window() || !load_renderer(config.renderer)) {
        cleanup();
        return 70;
      }

      running_ = true;
      [window_ makeKeyAndOrderFront:nil];
      [app activateIgnoringOtherApps:YES];

      while (running_) {
        @autoreleasepool {
          NSDate *until =
              [NSDate dateWithTimeIntervalSinceNow:0.015];
          NSEvent *event =
              [app nextEventMatchingMask:NSEventMaskAny
                                untilDate:until
                                   inMode:NSDefaultRunLoopMode
                                  dequeue:YES];
          if (event != nil) {
            [app sendEvent:event];
          }
          [app updateWindows];
          if (active_config_.pump_core_events) {
            active_config_.pump_core_events();
          }
        }
      }

      cleanup();
      return exit_code_;
    }
  }

  void emit_event(const std::string &event_json) override {
    if (webview_ == nil || !renderer_ready_) {
      pending_events_.push_back(event_json);
      return;
    }
    post_json_to_renderer(event_json);
  }

  void post_host_response(const std::string &response_json) override {
    post_json_to_renderer(response_json);
  }

  void post_bridge_success(int id, const nlohmann::ordered_json &data) {
    nlohmann::ordered_json out;
    out["id"] = id;
    out["ok"] = true;
    out["data"] = data;
    post_json_to_renderer(out.dump());
  }

  void post_bridge_error(int id, const char *code, const char *message) {
    nlohmann::ordered_json out;
    out["id"] = id;
    out["ok"] = false;
    out["code"] = code;
    out["message"] = message;
    post_json_to_renderer(out.dump());
  }

  void handle_script_message(NSString *message) {
    const std::string request_json = utf8_from_ns_string(message);
    if (request_json.empty()) {
      return;
    }

    try {
      auto parsed = nlohmann::json::parse(request_json);
      const std::string action = parsed.value("action", "");
      const int id = parsed.value("id", 0);
      if (action == "window.setMode") {
        std::string mode = "advanced";
        std::uint64_t mode_request = 0;
        if (parsed.contains("payload") && parsed["payload"].is_object()) {
          const auto &payload = parsed["payload"];
          if (payload.contains("mode") && payload["mode"].is_string()) {
            mode = payload["mode"].get<std::string>();
          }
          if (payload.contains("request") &&
              payload["request"].is_number_unsigned()) {
            mode_request = payload["request"].get<std::uint64_t>();
          } else if (payload.contains("request") &&
                     payload["request"].is_number_integer()) {
            const auto request_value = payload["request"].get<std::int64_t>();
            if (request_value > 0) {
              mode_request = static_cast<std::uint64_t>(request_value);
            }
          }
        }
        bool stale_window_mode_request = false;
        if (mode_request > 0) {
          if (mode_request < latest_window_mode_request_) {
            stale_window_mode_request = true;
          } else {
            latest_window_mode_request_ = mode_request;
          }
        }
        mode = mode == "minimal" ? "minimal" : "advanced";
        if (!stale_window_mode_request) {
          apply_window_mode(mode);
        }
        nlohmann::ordered_json out;
        out["id"] = id;
        out["ok"] = true;
        nlohmann::ordered_json data;
        data["ok"] = true;
        data["mode"] = current_window_mode_;
        out["data"] = data;
        post_json_to_renderer(out.dump());
        return;
      }
      if (action == "window.resizeForMode") {
        std::string mode = "advanced";
        std::uint64_t mode_request = 0;
        if (parsed.contains("payload") && parsed["payload"].is_object()) {
          const auto &payload = parsed["payload"];
          if (payload.contains("mode") && payload["mode"].is_string()) {
            mode = payload["mode"].get<std::string>();
          }
          if (payload.contains("request") &&
              payload["request"].is_number_unsigned()) {
            mode_request = payload["request"].get<std::uint64_t>();
          } else if (payload.contains("request") &&
                     payload["request"].is_number_integer()) {
            const auto request_value = payload["request"].get<std::int64_t>();
            if (request_value > 0) {
              mode_request = static_cast<std::uint64_t>(request_value);
            }
          }
        }
        mode = mode == "minimal" ? "minimal" : "advanced";
        if (mode_request > 0) {
          if (mode_request < latest_window_mode_request_) {
            nlohmann::ordered_json data;
            data["ok"] = true;
            data["mode"] = current_window_mode_;
            post_bridge_success(id, data);
            return;
          }
          latest_window_mode_request_ = mode_request;
        }
        apply_window_mode_once(mode);
        nlohmann::ordered_json data;
        data["ok"] = true;
        data["mode"] = current_window_mode_;
        post_bridge_success(id, data);
        return;
      }
      if (action == "window.minimize") {
        if (window_ != nil) {
          [window_ miniaturize:nil];
        }
        nlohmann::ordered_json data;
        data["ok"] = true;
        post_bridge_success(id, data);
        return;
      }
      if (action == "window.startDrag") {
        NSEvent *event = [[NSApplication sharedApplication] currentEvent];
        if (window_ != nil && event != nil) {
          [window_ performWindowDragWithEvent:event];
        }
        nlohmann::ordered_json data;
        data["ok"] = true;
        post_bridge_success(id, data);
        return;
      }
      if (action == "shell.openExternal") {
        std::string url;
        if (parsed.contains("payload") && parsed["payload"].is_object()) {
          const auto &payload = parsed["payload"];
          if (payload.contains("url") && payload["url"].is_string()) {
            url = payload["url"].get<std::string>();
          }
        }
        if (!is_supported_external_url(url)) {
          post_bridge_error(id, "invalid_url",
                            "Only http and https URLs can be opened.");
          return;
        }
        NSURL *external_url = [NSURL URLWithString:ns_string(url)];
        if (external_url == nil ||
            ![[NSWorkspace sharedWorkspace] openURL:external_url]) {
          post_bridge_error(id, "open_external_failed",
                            "Unable to open the URL in the default browser.");
          return;
        }
        nlohmann::ordered_json data;
        data["ok"] = true;
        post_bridge_success(id, data);
        return;
      }
      if (action == "window.requestClose") {
        request_close_decision();
        nlohmann::ordered_json data;
        data["ok"] = true;
        post_bridge_success(id, data);
        return;
      }
      if (action == "window.resolveClosePrompt") {
        apply_close_resolution(
            exv::ui_shell::parse_close_prompt_resolution(request_json));
        nlohmann::ordered_json out;
        nlohmann::ordered_json data;
        data["ok"] = true;
        out["id"] = id;
        out["ok"] = true;
        out["data"] = data;
        post_json_to_renderer(out.dump());
        return;
      }
    } catch (const nlohmann::json::exception &) {
      // Fall through to existing handler_ path on parse failure.
    }

    const std::string response_json =
        handler_ ? handler_(request_json)
                 : R"({"id":0,"ok":false,"code":"host_unavailable","message":"Desktop host bridge is not ready"})";
    if (!response_json.empty()) {
      post_json_to_renderer(response_json);
    }
  }

  void on_navigation_finished() {
    renderer_ready_ = true;
    flush_pending_events();
  }

  // Returns true to permit the window close, false to intercept and route
  // through the renderer-driven close prompt.  Cocoa calls this from the
  // window delegate's `windowShouldClose:` hook BEFORE the window is
  // ordered out, so the prompt is shown while the window is still on
  // screen.  `windowWillClose:` then handles run-loop teardown only.
  bool should_close_window() {
    if (force_quit_) {
      return true;
    }
    request_close_decision();
    return false;
  }

  void close_from_window() {
    running_ = false;
    exit_code_ = 0;
  }

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

  void request_close_decision() {
    if (close_prompt_pending_) {
      show_from_status_item();
      return;
    }
    if (const auto remembered_action =
            exv::ui_shell::read_close_preference(active_config_.state_dir)) {
      apply_close_resolution({*remembered_action, false});
      return;
    }
    close_prompt_pending_ = true;
    emit_event(R"({"type":"close-request","data":{}})");
  }

  void apply_close_resolution(
      const exv::ui_shell::ClosePromptResolution &resolution) {
    close_prompt_pending_ = false;
    if (resolution.remember) {
      exv::ui_shell::write_close_preference(active_config_.state_dir,
                                                resolution.action);
    }

    if (resolution.action == "tray") {
      if (window_ != nil) {
        [window_ orderOut:nil];
      }
    } else if (resolution.action == "quit") {
      quit_from_status_item();
    } else {
      show_from_status_item();
    }
  }

  void apply_window_mode_once(const std::string &mode) {
    current_window_mode_ = mode == "minimal" ? "minimal" : "advanced";
    if (window_ == nil) {
      return;
    }
    const auto bounds = current_window_mode_ == "minimal"
                            ? exv::ui_shell::kElectronMinimalWindowBounds
                            : exv::ui_shell::kElectronAdvancedWindowBounds;
    NSRect frame = [window_ frame];
    frame.size = NSMakeSize(bounds.width, bounds.height);
    [window_ setContentMinSize:NSMakeSize(bounds.width, bounds.height)];
    [window_ setContentMaxSize:NSMakeSize(bounds.width, bounds.height)];
    [window_ setFrame:frame display:YES animate:NO];
  }

  void apply_window_mode(const std::string &mode) {
    apply_window_mode_once(mode);
  }

private:
  bool create_window() {
    const auto bounds = wkwebview_default_window_bounds();
    const NSRect frame = NSMakeRect(0, 0, bounds.width, bounds.height);
    window_ = [[NSWindow alloc]
        initWithContentRect:frame
                  styleMask:(NSWindowStyleMaskTitled |
                             NSWindowStyleMaskClosable |
                             NSWindowStyleMaskMiniaturizable |
                             NSWindowStyleMaskFullSizeContentView)
                    backing:NSBackingStoreBuffered
                      defer:NO];
    if (window_ == nil) {
      return false;
    }
    [window_ setTitle:@"EXV"];
    [window_ setTitlebarAppearsTransparent:YES];
    [window_ setTitleVisibility:NSWindowTitleHidden];
    [window_ setMovableByWindowBackground:YES];
    [window_ setOpaque:NO];
    [window_ setBackgroundColor:[NSColor clearColor]];
    [window_ center];
    [window_ setContentMinSize:NSMakeSize(bounds.width, bounds.height)];
    [window_ setContentMaxSize:NSMakeSize(bounds.width, bounds.height)];

    window_delegate_ = [[ExvWindowDelegate alloc] initWithOwner:this];
    [window_ setDelegate:window_delegate_];

    WKWebViewConfiguration *configuration =
        [[[WKWebViewConfiguration alloc] init] autorelease];
    content_controller_ = [[WKUserContentController alloc] init];
    configuration.userContentController = content_controller_;

    script_handler_ = [[ExvScriptMessageHandler alloc] initWithOwner:this];
    [content_controller_ addScriptMessageHandler:script_handler_
                                           name:@"exvHost"];
    WKUserScript *user_script =
        [[[WKUserScript alloc] initWithSource:bridge_script()
                                injectionTime:WKUserScriptInjectionTimeAtDocumentStart
                             forMainFrameOnly:NO] autorelease];
    [content_controller_ addUserScript:user_script];

    webview_ = [[WKWebView alloc] initWithFrame:[[window_ contentView] bounds]
                                  configuration:configuration];
    if (webview_ == nil) {
      return false;
    }
    [webview_ setValue:@NO forKey:@"drawsBackground"];
    [webview_ setAutoresizingMask:(NSViewWidthSizable | NSViewHeightSizable)];

    navigation_delegate_ =
        [[ExvNavigationDelegate alloc] initWithOwner:this];
    [webview_ setNavigationDelegate:navigation_delegate_];
    ui_delegate_ = [[ExvUIDelegate alloc] init];
    [webview_ setUIDelegate:ui_delegate_];
    [window_ setContentView:webview_];
    return true;
  }

  bool load_renderer(const exv::ui_shell::RendererAssets &renderer) {
    NSURL *url = renderer_url(renderer);
    if (url == nil) {
      return false;
    }

    if (renderer.kind == exv::ui_shell::RendererAssetKind::DevServer) {
      [webview_ loadRequest:[NSURLRequest requestWithURL:url]];
      return true;
    }

    NSURL *read_access = renderer_read_access_url(renderer);
    if (read_access == nil) {
      return false;
    }
    [webview_ loadFileURL:url allowingReadAccessToURL:read_access];
    return true;
  }

  void post_json_to_renderer(const std::string &json) {
    if (webview_ == nil) {
      pending_events_.push_back(json);
      return;
    }
    const std::string script =
        "window.__exvHostReceive && window.__exvHostReceive(" +
        json + ");";
    [webview_ evaluateJavaScript:ns_string(script) completionHandler:nil];
  }

  void flush_pending_events() {
    std::vector<std::string> events;
    events.swap(pending_events_);
    for (const std::string &event_json : events) {
      post_json_to_renderer(event_json);
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
    status_target_ = [[ExvStatusItemTarget alloc] initWithOwner:this];
    status_item_ = [[[NSStatusBar systemStatusBar]
        statusItemWithLength:NSSquareStatusItemLength] retain];
    [[status_item_ button] setImage:status_icon()];
    [[status_item_ button] setToolTip:@"EXV"];

    status_menu_ = [[NSMenu alloc] initWithTitle:@"EXV"];
    [status_menu_ addItemWithTitle:@"显示 EXV"
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

  void cleanup() {
    destroy_status_item();
    if (content_controller_ != nil) {
      [content_controller_ removeScriptMessageHandlerForName:@"exvHost"];
    }
    if (webview_ != nil) {
      [webview_ setNavigationDelegate:nil];
      [webview_ setUIDelegate:nil];
    }
    if (window_ != nil) {
      [window_ setDelegate:nil];
      [window_ close];
    }

    [ui_delegate_ release];
    ui_delegate_ = nil;
    [navigation_delegate_ release];
    navigation_delegate_ = nil;
    [script_handler_ release];
    script_handler_ = nil;
    [webview_ release];
    webview_ = nil;
    [content_controller_ release];
    content_controller_ = nil;
    [window_delegate_ release];
    window_delegate_ = nil;
    [window_ release];
    window_ = nil;
    renderer_ready_ = false;
  }

  exv::ui_shell::HostMessageHandler handler_;
  exv::ui_shell::UiWindowConfig active_config_;
  NSWindow *window_ = nil;
  WKWebView *webview_ = nil;
  WKUserContentController *content_controller_ = nil;
  ExvScriptMessageHandler *script_handler_ = nil;
  ExvWindowDelegate *window_delegate_ = nil;
  ExvNavigationDelegate *navigation_delegate_ = nil;
  ExvUIDelegate *ui_delegate_ = nil;
  NSStatusItem *status_item_ = nil;
  NSMenu *status_menu_ = nil;
  ExvStatusItemTarget *status_target_ = nil;
  std::vector<std::string> pending_events_;
  bool running_ = false;
  bool renderer_ready_ = false;
  bool force_quit_ = false;
  bool close_prompt_pending_ = false;
  std::uint64_t latest_window_mode_request_ = 0;
  std::string current_window_mode_ = "advanced";
  int exit_code_ = 70;
};
#else
class WkWebViewWindow final : public exv::ui_shell::UiWindow {
public:
  void set_message_handler(exv::ui_shell::HostMessageHandler handler) override {
    handler_ = std::move(handler);
  }

  int run(const exv::ui_shell::UiWindowConfig &) override { return 70; }

  void emit_event(const std::string &event_json) override {
    last_event_json_ = event_json;
  }

private:
  exv::ui_shell::HostMessageHandler handler_;
  std::string last_event_json_;
};
#endif

exv::ui_shell::WindowBounds wkwebview_default_window_bounds() noexcept {
  return exv::ui_shell::kElectronAdvancedWindowBounds;
}

std::string dispatch_wkwebview_host_message(
    const std::string &message_json,
    const exv::ui_shell::CoreRpcInvoker &invoke_core) {
  return exv::ui_shell::handle_host_request(message_json, invoke_core);
}

std::unique_ptr<exv::ui_shell::UiWindow> create_wk_webview_window() {
  return std::make_unique<WkWebViewWindow>();
}

} // namespace exv::platform::darwin::ui_shell

#if defined(EXV_BUILD_UI_SHELL)
@implementation ExvScriptMessageHandler
- (instancetype)initWithOwner:
    (exv::platform::darwin::ui_shell::WkWebViewWindow *)owner {
  self = [super init];
  if (self != nil) {
    owner_ = owner;
  }
  return self;
}

- (void)userContentController:(WKUserContentController *)userContentController
      didReceiveScriptMessage:(WKScriptMessage *)message {
  (void)userContentController;
  if (owner_ == nullptr) {
    return;
  }
  if ([message.body isKindOfClass:[NSString class]]) {
    owner_->handle_script_message((NSString *)message.body);
  }
}
@end

@implementation ExvWindowDelegate
- (instancetype)initWithOwner:
    (exv::platform::darwin::ui_shell::WkWebViewWindow *)owner {
  self = [super init];
  if (self != nil) {
    owner_ = owner;
  }
  return self;
}

- (BOOL)windowShouldClose:(NSWindow *)sender {
  (void)sender;
  if (owner_ != nullptr) {
    return owner_->should_close_window() ? YES : NO;
  }
  return YES;
}

- (void)windowWillClose:(NSNotification *)notification {
  (void)notification;
  if (owner_ != nullptr) {
    owner_->close_from_window();
  }
}
@end

@implementation ExvNavigationDelegate
- (instancetype)initWithOwner:
    (exv::platform::darwin::ui_shell::WkWebViewWindow *)owner {
  self = [super init];
  if (self != nil) {
    owner_ = owner;
  }
  return self;
}

- (void)webView:(WKWebView *)webView
    didFinishNavigation:(WKNavigation *)navigation {
  (void)webView;
  (void)navigation;
  if (owner_ != nullptr) {
    owner_->on_navigation_finished();
  }
}
@end

@implementation ExvUIDelegate
- (void)webView:(WKWebView *)webView
    runJavaScriptAlertPanelWithMessage:(NSString *)message
                      initiatedByFrame:(WKFrameInfo *)frame
                     completionHandler:(void (^)(void))completionHandler {
  (void)webView;
  (void)frame;
  NSAlert *alert = [[[NSAlert alloc] init] autorelease];
  [alert setMessageText:message ?: @"EXV"];
  [alert addButtonWithTitle:@"OK"];
  [alert runModal];
  completionHandler();
}

- (void)webView:(WKWebView *)webView
    runJavaScriptConfirmPanelWithMessage:(NSString *)message
                        initiatedByFrame:(WKFrameInfo *)frame
                       completionHandler:(void (^)(BOOL result))completionHandler {
  (void)webView;
  (void)frame;
  NSAlert *alert = [[[NSAlert alloc] init] autorelease];
  [alert setMessageText:message ?: @"EXV"];
  [alert addButtonWithTitle:@"OK"];
  [alert addButtonWithTitle:@"Cancel"];
  completionHandler([alert runModal] == NSAlertFirstButtonReturn);
}

- (void)webView:(WKWebView *)webView
    runJavaScriptTextInputPanelWithPrompt:(NSString *)prompt
                              defaultText:(NSString *)defaultText
                         initiatedByFrame:(WKFrameInfo *)frame
                        completionHandler:
                            (void (^)(NSString *_Nullable result))completionHandler {
  (void)webView;
  (void)frame;
  NSAlert *alert = [[[NSAlert alloc] init] autorelease];
  [alert setMessageText:prompt ?: @"EXV"];
  NSTextField *input =
      [[[NSTextField alloc] initWithFrame:NSMakeRect(0, 0, 320, 24)] autorelease];
  [input setStringValue:defaultText ?: @""];
  [alert setAccessoryView:input];
  [alert addButtonWithTitle:@"OK"];
  [alert addButtonWithTitle:@"Cancel"];
  if ([alert runModal] == NSAlertFirstButtonReturn) {
    completionHandler([input stringValue]);
  } else {
    completionHandler(nil);
  }
}
@end

@implementation ExvStatusItemTarget
- (instancetype)initWithOwner:
    (exv::platform::darwin::ui_shell::WkWebViewWindow *)owner {
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
#endif
