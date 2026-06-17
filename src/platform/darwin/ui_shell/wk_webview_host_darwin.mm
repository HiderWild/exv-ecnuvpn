#include "app/ui_shell/host_bridge.hpp"
#include "app/ui_shell/ui_window.hpp"

#if defined(EXV_BUILD_UI_SHELL)
#import <Cocoa/Cocoa.h>
#import <WebKit/WebKit.h>
#endif

#include <filesystem>
#include <memory>
#include <string>
#include <utility>
#include <vector>

namespace ecnuvpn::platform::darwin::ui_shell {
class WkWebViewWindow;
}

#if defined(EXV_BUILD_UI_SHELL)
@interface EcnuVpnScriptMessageHandler : NSObject <WKScriptMessageHandler> {
  ecnuvpn::platform::darwin::ui_shell::WkWebViewWindow *owner_;
}
- (instancetype)initWithOwner:
    (ecnuvpn::platform::darwin::ui_shell::WkWebViewWindow *)owner;
@end

@interface EcnuVpnWindowDelegate : NSObject <NSWindowDelegate> {
  ecnuvpn::platform::darwin::ui_shell::WkWebViewWindow *owner_;
}
- (instancetype)initWithOwner:
    (ecnuvpn::platform::darwin::ui_shell::WkWebViewWindow *)owner;
@end

@interface EcnuVpnNavigationDelegate : NSObject <WKNavigationDelegate> {
  ecnuvpn::platform::darwin::ui_shell::WkWebViewWindow *owner_;
}
- (instancetype)initWithOwner:
    (ecnuvpn::platform::darwin::ui_shell::WkWebViewWindow *)owner;
@end

@interface EcnuVpnUIDelegate : NSObject <WKUIDelegate>
@end
#endif

namespace ecnuvpn::platform::darwin::ui_shell {
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

NSURL *renderer_url(const ecnuvpn::ui_shell::RendererAssets &renderer) {
  if (renderer.kind == ecnuvpn::ui_shell::RendererAssetKind::DevServer) {
    return [NSURL URLWithString:ns_string(renderer.location)];
  }

  const std::filesystem::path absolute =
      std::filesystem::absolute(std::filesystem::path(renderer.location));
  return [NSURL fileURLWithPath:ns_string(absolute.string())];
}

NSURL *renderer_read_access_url(
    const ecnuvpn::ui_shell::RendererAssets &renderer) {
  if (renderer.kind == ecnuvpn::ui_shell::RendererAssetKind::DevServer) {
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
  if (window.ecnuVpn || !window.webkit || !window.webkit.messageHandlers || !window.webkit.messageHandlers.ecnuVpnHost) return;
  let nextId = 1;
  const pending = new Map();
  const subscribers = new Set();
  function rpc(action, payload) {
    const id = nextId++;
    const request = { id, action, payload: payload ?? {} };
    window.webkit.messageHandlers.ecnuVpnHost.postMessage(JSON.stringify(request));
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
    });
  }
  function unsupported(name) {
    return Promise.reject(new Error(`${name} is not available in the native WebView shell yet.`));
  }
  window.__ecnuVpnHostReceive = (message) => {
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
  window.ecnuVpn = {
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
      status: () => unsupported('cli.status'),
      install: () => unsupported('cli.install'),
      uninstall: () => unsupported('cli.uninstall'),
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
      setMode: () => Promise.resolve(),
      resolveClosePrompt: () => Promise.resolve(),
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

#if defined(EXV_BUILD_UI_SHELL)
class WkWebViewWindow final : public ecnuvpn::ui_shell::UiWindow {
public:
  ~WkWebViewWindow() override { cleanup(); }

  void set_message_handler(ecnuvpn::ui_shell::HostMessageHandler handler) override {
    handler_ = std::move(handler);
  }

  int run(const ecnuvpn::ui_shell::UiWindowConfig &config) override {
    if (![NSThread isMainThread]) {
      return 70;
    }

    @autoreleasepool {
      active_config_ = config;
      exit_code_ = 0;
      renderer_ready_ = false;

      NSApplication *app = [NSApplication sharedApplication];
      [app setActivationPolicy:NSApplicationActivationPolicyRegular];

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

  void handle_script_message(NSString *message) {
    const std::string request_json = utf8_from_ns_string(message);
    if (request_json.empty()) {
      return;
    }

    const std::string response_json =
        handler_ ? handler_(request_json)
                 : R"({"id":0,"ok":false,"code":"host_unavailable","message":"Desktop host bridge is not ready"})";
    post_json_to_renderer(response_json);
  }

  void on_navigation_finished() {
    renderer_ready_ = true;
    flush_pending_events();
  }

  void close_from_window() {
    running_ = false;
    exit_code_ = 0;
  }

private:
  bool create_window() {
    const NSRect frame = NSMakeRect(0, 0, 1180, 760);
    window_ = [[NSWindow alloc]
        initWithContentRect:frame
                  styleMask:(NSWindowStyleMaskTitled |
                             NSWindowStyleMaskClosable |
                             NSWindowStyleMaskMiniaturizable |
                             NSWindowStyleMaskResizable)
                    backing:NSBackingStoreBuffered
                      defer:NO];
    if (window_ == nil) {
      return false;
    }
    [window_ setTitle:@"ECNU VPN"];
    [window_ center];

    window_delegate_ = [[EcnuVpnWindowDelegate alloc] initWithOwner:this];
    [window_ setDelegate:window_delegate_];

    WKWebViewConfiguration *configuration =
        [[[WKWebViewConfiguration alloc] init] autorelease];
    content_controller_ = [[WKUserContentController alloc] init];
    configuration.userContentController = content_controller_;

    script_handler_ = [[EcnuVpnScriptMessageHandler alloc] initWithOwner:this];
    [content_controller_ addScriptMessageHandler:script_handler_
                                           name:@"ecnuVpnHost"];
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
    [webview_ setAutoresizingMask:(NSViewWidthSizable | NSViewHeightSizable)];

    navigation_delegate_ =
        [[EcnuVpnNavigationDelegate alloc] initWithOwner:this];
    [webview_ setNavigationDelegate:navigation_delegate_];
    ui_delegate_ = [[EcnuVpnUIDelegate alloc] init];
    [webview_ setUIDelegate:ui_delegate_];
    [window_ setContentView:webview_];
    return true;
  }

  bool load_renderer(const ecnuvpn::ui_shell::RendererAssets &renderer) {
    NSURL *url = renderer_url(renderer);
    if (url == nil) {
      return false;
    }

    if (renderer.kind == ecnuvpn::ui_shell::RendererAssetKind::DevServer) {
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
        "window.__ecnuVpnHostReceive && window.__ecnuVpnHostReceive(" +
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

  void cleanup() {
    if (content_controller_ != nil) {
      [content_controller_ removeScriptMessageHandlerForName:@"ecnuVpnHost"];
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

  ecnuvpn::ui_shell::HostMessageHandler handler_;
  ecnuvpn::ui_shell::UiWindowConfig active_config_;
  NSWindow *window_ = nil;
  WKWebView *webview_ = nil;
  WKUserContentController *content_controller_ = nil;
  EcnuVpnScriptMessageHandler *script_handler_ = nil;
  EcnuVpnWindowDelegate *window_delegate_ = nil;
  EcnuVpnNavigationDelegate *navigation_delegate_ = nil;
  EcnuVpnUIDelegate *ui_delegate_ = nil;
  std::vector<std::string> pending_events_;
  bool running_ = false;
  bool renderer_ready_ = false;
  int exit_code_ = 70;
};
#else
class WkWebViewWindow final : public ecnuvpn::ui_shell::UiWindow {
public:
  void set_message_handler(ecnuvpn::ui_shell::HostMessageHandler handler) override {
    handler_ = std::move(handler);
  }

  int run(const ecnuvpn::ui_shell::UiWindowConfig &) override { return 70; }

  void emit_event(const std::string &event_json) override {
    last_event_json_ = event_json;
  }

private:
  ecnuvpn::ui_shell::HostMessageHandler handler_;
  std::string last_event_json_;
};
#endif

std::string dispatch_wkwebview_host_message(
    const std::string &message_json,
    const ecnuvpn::ui_shell::CoreRpcInvoker &invoke_core) {
  return ecnuvpn::ui_shell::handle_host_request(message_json, invoke_core);
}

std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_wk_webview_window() {
  return std::make_unique<WkWebViewWindow>();
}

} // namespace ecnuvpn::platform::darwin::ui_shell

#if defined(EXV_BUILD_UI_SHELL)
@implementation EcnuVpnScriptMessageHandler
- (instancetype)initWithOwner:
    (ecnuvpn::platform::darwin::ui_shell::WkWebViewWindow *)owner {
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

@implementation EcnuVpnWindowDelegate
- (instancetype)initWithOwner:
    (ecnuvpn::platform::darwin::ui_shell::WkWebViewWindow *)owner {
  self = [super init];
  if (self != nil) {
    owner_ = owner;
  }
  return self;
}

- (void)windowWillClose:(NSNotification *)notification {
  (void)notification;
  if (owner_ != nullptr) {
    owner_->close_from_window();
  }
}
@end

@implementation EcnuVpnNavigationDelegate
- (instancetype)initWithOwner:
    (ecnuvpn::platform::darwin::ui_shell::WkWebViewWindow *)owner {
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

@implementation EcnuVpnUIDelegate
- (void)webView:(WKWebView *)webView
    runJavaScriptAlertPanelWithMessage:(NSString *)message
                      initiatedByFrame:(WKFrameInfo *)frame
                     completionHandler:(void (^)(void))completionHandler {
  (void)webView;
  (void)frame;
  NSAlert *alert = [[[NSAlert alloc] init] autorelease];
  [alert setMessageText:message ?: @"ECNU VPN"];
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
  [alert setMessageText:message ?: @"ECNU VPN"];
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
  [alert setMessageText:prompt ?: @"ECNU VPN"];
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
#endif
