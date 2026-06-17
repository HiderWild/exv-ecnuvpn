#include "platform/win32/ui_shell/webview2_host_win32.hpp"

#include "platform/win32/ui_shell/webview2_runtime_win32.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <filesystem>
#include <memory>
#include <string>
#include <utility>
#include <vector>

#if defined(EXV_BUILD_UI_SHELL)
#include <WebView2.h>
#include <objbase.h>
#endif

namespace ecnuvpn::platform::win32::ui_shell {

namespace {

constexpr char kPackagedRendererHost[] = "appassets.ecnu-vpn.invalid";
constexpr wchar_t kPackagedRendererHostWide[] =
    L"appassets.ecnu-vpn.invalid";
constexpr DWORD kFixedWindowStyle =
    WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

std::wstring wide_from_utf8(const std::string &value) {
  if (value.empty()) {
    return {};
  }
  const int required =
      MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
                          static_cast<int>(value.size()), nullptr, 0);
  if (required <= 0) {
    return {};
  }
  std::wstring out(static_cast<std::size_t>(required), L'\0');
  MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
                      static_cast<int>(value.size()), out.data(), required);
  return out;
}

std::string utf8_from_wide(const wchar_t *value) {
  if (!value) {
    return {};
  }
  const int required =
      WideCharToMultiByte(CP_UTF8, 0, value, -1, nullptr, 0, nullptr, nullptr);
  if (required <= 1) {
    return {};
  }
  std::string out(static_cast<std::size_t>(required), '\0');
  WideCharToMultiByte(CP_UTF8, 0, value, -1, out.data(), required, nullptr,
                      nullptr);
  if (!out.empty() && out.back() == '\0') {
    out.pop_back();
  }
  return out;
}

bool is_file_uri_path_byte_safe(unsigned char value) {
  return (value >= 'A' && value <= 'Z') || (value >= 'a' && value <= 'z') ||
         (value >= '0' && value <= '9') || value == '/' || value == ':' ||
         value == '-' || value == '_' || value == '.' || value == '~';
}

std::string percent_encode_file_uri_path(const std::string &value) {
  static constexpr char kHex[] = "0123456789ABCDEF";

  std::string out;
  out.reserve(value.size());
  for (unsigned char byte : value) {
    if (is_file_uri_path_byte_safe(byte)) {
      out.push_back(static_cast<char>(byte));
      continue;
    }
    out.push_back('%');
    out.push_back(kHex[byte >> 4]);
    out.push_back(kHex[byte & 0x0F]);
  }
  return out;
}

#if defined(EXV_BUILD_UI_SHELL)

std::wstring temp_bootstrapper_path() {
  wchar_t temp_dir[MAX_PATH] = {};
  const DWORD dir_length = GetTempPathW(MAX_PATH, temp_dir);
  if (dir_length == 0 || dir_length >= MAX_PATH) {
    return {};
  }
  wchar_t temp_file[MAX_PATH] = {};
  if (GetTempFileNameW(temp_dir, L"wv2", 0, temp_file) == 0) {
    return {};
  }
  return temp_file;
}

template <typename T> class ComPtr {
public:
  ComPtr() = default;
  ~ComPtr() { reset(); }

  ComPtr(const ComPtr &) = delete;
  ComPtr &operator=(const ComPtr &) = delete;

  T *get() const { return ptr_; }
  T *operator->() const { return ptr_; }
  explicit operator bool() const { return ptr_ != nullptr; }

  T **put() {
    reset();
    return &ptr_;
  }

  void attach(T *value) {
    reset();
    ptr_ = value;
  }

  void copy_from(T *value) {
    reset();
    ptr_ = value;
    if (ptr_) {
      ptr_->AddRef();
    }
  }

  void reset() {
    if (ptr_) {
      ptr_->Release();
      ptr_ = nullptr;
    }
  }

private:
  T *ptr_ = nullptr;
};

class WebView2Window;

class EnvironmentCompletedHandler final
    : public ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler {
public:
  explicit EnvironmentCompletedHandler(WebView2Window *owner) : owner_(owner) {}

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void **object) override;
  ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs_); }
  ULONG STDMETHODCALLTYPE Release() override {
    const ULONG refs = InterlockedDecrement(&refs_);
    if (refs == 0) {
      delete this;
    }
    return refs;
  }
  HRESULT STDMETHODCALLTYPE Invoke(HRESULT error_code,
                                   ICoreWebView2Environment *result) override;

private:
  volatile ULONG refs_ = 1;
  WebView2Window *owner_ = nullptr;
};

class ControllerCompletedHandler final
    : public ICoreWebView2CreateCoreWebView2ControllerCompletedHandler {
public:
  explicit ControllerCompletedHandler(WebView2Window *owner) : owner_(owner) {}

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void **object) override;
  ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs_); }
  ULONG STDMETHODCALLTYPE Release() override {
    const ULONG refs = InterlockedDecrement(&refs_);
    if (refs == 0) {
      delete this;
    }
    return refs;
  }
  HRESULT STDMETHODCALLTYPE Invoke(HRESULT error_code,
                                   ICoreWebView2Controller *result) override;

private:
  volatile ULONG refs_ = 1;
  WebView2Window *owner_ = nullptr;
};

class WebMessageReceivedHandler final
    : public ICoreWebView2WebMessageReceivedEventHandler {
public:
  explicit WebMessageReceivedHandler(WebView2Window *owner) : owner_(owner) {}

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void **object) override;
  ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs_); }
  ULONG STDMETHODCALLTYPE Release() override {
    const ULONG refs = InterlockedDecrement(&refs_);
    if (refs == 0) {
      delete this;
    }
    return refs;
  }
  HRESULT STDMETHODCALLTYPE Invoke(ICoreWebView2 *sender,
                                   ICoreWebView2WebMessageReceivedEventArgs *args) override;

private:
  volatile ULONG refs_ = 1;
  WebView2Window *owner_ = nullptr;
};

class WebView2Window final : public ecnuvpn::ui_shell::UiWindow {
public:
  ~WebView2Window() override {
    if (webview_ && web_message_token_.value != 0) {
      webview_->remove_WebMessageReceived(web_message_token_);
    }
  }

  void set_message_handler(ecnuvpn::ui_shell::HostMessageHandler handler) override {
    handler_ = std::move(handler);
  }

  int run(const ecnuvpn::ui_shell::UiWindowConfig &config) override {
    active_config_ = config;
    if (!ensure_runtime_available()) {
      return 70;
    }

    const HRESULT coinit = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    const bool coinit_ok = SUCCEEDED(coinit);
    if (!coinit_ok && coinit != RPC_E_CHANGED_MODE) {
      return 70;
    }

    if (!create_window()) {
      if (coinit_ok) {
        CoUninitialize();
      }
      return 70;
    }

    running_ = true;
    ShowWindow(hwnd_, SW_SHOW);
    UpdateWindow(hwnd_);

    auto *handler = new EnvironmentCompletedHandler(this);
    const HRESULT created = CreateCoreWebView2EnvironmentWithOptions(
        nullptr, nullptr, nullptr, handler);
    handler->Release();
    if (FAILED(created)) {
      DestroyWindow(hwnd_);
      running_ = false;
      exit_code_ = 70;
    }

    MSG message{};
    while (running_) {
      while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
        if (message.message == WM_QUIT) {
          running_ = false;
          break;
        }
        TranslateMessage(&message);
        DispatchMessageW(&message);
      }
      if (active_config_.pump_core_events) {
        active_config_.pump_core_events();
      }
      Sleep(15);
    }

    if (hwnd_) {
      DestroyWindow(hwnd_);
      hwnd_ = nullptr;
    }
    if (coinit_ok) {
      CoUninitialize();
    }
    return exit_code_;
  }

  void emit_event(const std::string &event_json) override {
    if (!webview_) {
      pending_events_.push_back(event_json);
      return;
    }
    const std::wstring wide_event = wide_from_utf8(event_json);
    webview_->PostWebMessageAsJson(wide_event.c_str());
  }

  void on_environment_created(HRESULT error_code,
                              ICoreWebView2Environment *environment) {
    if (FAILED(error_code) || !environment || !hwnd_) {
      fail_and_close(L"Unable to create the WebView2 environment.");
      return;
    }
    environment_.copy_from(environment);
    auto *handler = new ControllerCompletedHandler(this);
    const HRESULT hr = environment_->CreateCoreWebView2Controller(hwnd_, handler);
    handler->Release();
    if (FAILED(hr)) {
      fail_and_close(L"Unable to create the WebView2 controller.");
    }
  }

  void on_controller_created(HRESULT error_code,
                             ICoreWebView2Controller *controller) {
    if (FAILED(error_code) || !controller) {
      fail_and_close(L"Unable to initialize the WebView2 controller.");
      return;
    }

    controller_.copy_from(controller);
    ICoreWebView2 *raw_webview = nullptr;
    if (FAILED(controller_->get_CoreWebView2(&raw_webview)) || !raw_webview) {
      fail_and_close(L"Unable to access the WebView2 instance.");
      return;
    }
    webview_.attach(raw_webview);

    resize_webview();
    if (!configure_packaged_renderer_origin()) {
      fail_and_close(L"Unable to map the packaged renderer assets.");
      return;
    }
    install_renderer_bridge();

    auto *message_handler = new WebMessageReceivedHandler(this);
    const HRESULT add_result =
        webview_->add_WebMessageReceived(message_handler, &web_message_token_);
    message_handler->Release();
    if (FAILED(add_result)) {
      fail_and_close(L"Unable to register the WebView2 message bridge.");
      return;
    }

    const std::wstring uri = webview2_renderer_uri(active_config_.renderer);
    if (uri.empty() || FAILED(webview_->Navigate(uri.c_str()))) {
      fail_and_close(L"Unable to load the packaged renderer.");
      return;
    }

    exit_code_ = 0;
    flush_pending_events();
  }

  HRESULT on_web_message(ICoreWebView2WebMessageReceivedEventArgs *args) {
    if (!args || !webview_) {
      return E_INVALIDARG;
    }

    LPWSTR raw_message = nullptr;
    HRESULT message_result = args->TryGetWebMessageAsString(&raw_message);
    if (FAILED(message_result) || !raw_message) {
      message_result = args->get_WebMessageAsJson(&raw_message);
    }
    if (FAILED(message_result) || !raw_message) {
      return message_result;
    }

    std::string request_json = utf8_from_wide(raw_message);
    CoTaskMemFree(raw_message);
    if (request_json.empty()) {
      return S_OK;
    }

    std::string response_json =
        handler_ ? handler_(request_json)
                 : R"({"id":0,"ok":false,"code":"host_unavailable","message":"Desktop host bridge is not ready"})";
    const std::wstring wide_response = wide_from_utf8(response_json);
    return webview_->PostWebMessageAsJson(wide_response.c_str());
  }

  void resize_webview() {
    if (!controller_ || !hwnd_) {
      return;
    }
    RECT bounds{};
    GetClientRect(hwnd_, &bounds);
    controller_->put_Bounds(bounds);
  }

private:
  bool configure_packaged_renderer_origin() {
    if (active_config_.renderer.kind ==
        ecnuvpn::ui_shell::RendererAssetKind::DevServer) {
      return true;
    }

    ComPtr<ICoreWebView2_3> webview3;
    const HRESULT interface_result = webview_->QueryInterface(
        IID_ICoreWebView2_3, reinterpret_cast<void **>(webview3.put()));
    if (FAILED(interface_result) || !webview3) {
      return false;
    }

    const std::wstring folder =
        webview2_packaged_renderer_folder(active_config_.renderer);
    if (folder.empty()) {
      return false;
    }
    return SUCCEEDED(webview3->SetVirtualHostNameToFolderMapping(
        kPackagedRendererHostWide, folder.c_str(),
        COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY));
  }

  bool ensure_runtime_available() {
    if (detect_webview2_runtime().installed) {
      return true;
    }

    const int choice = MessageBoxW(
        nullptr,
        L"Microsoft Edge WebView2 Runtime is required. Download and install "
        L"the Evergreen Runtime now?",
        L"ECNU VPN", MB_ICONQUESTION | MB_YESNO | MB_DEFBUTTON2);
    if (choice != IDYES) {
      return false;
    }

    const std::wstring installer_path = temp_bootstrapper_path();
    if (installer_path.empty()) {
      return false;
    }
    const std::string installer_path_utf8 = utf8_from_wide(installer_path.c_str());
    return run_webview2_evergreen_bootstrapper(
               "https://go.microsoft.com/fwlink/?linkid=2124703",
               installer_path_utf8) &&
           detect_webview2_runtime().installed;
  }

  bool create_window() {
    const auto bounds = webview2_default_window_bounds();
    instance_ = GetModuleHandleW(nullptr);
    WNDCLASSW window_class{};
    window_class.lpfnWndProc = &WebView2Window::window_proc;
    window_class.hInstance = instance_;
    window_class.lpszClassName = L"ECNUVPNWebViewShellWindow";
    window_class.hCursor = LoadCursorW(nullptr, MAKEINTRESOURCEW(32512));
    RegisterClassW(&window_class);

    hwnd_ = CreateWindowExW(0, window_class.lpszClassName, L"ECNU VPN",
                            kFixedWindowStyle, CW_USEDEFAULT, CW_USEDEFAULT,
                            bounds.width, bounds.height, nullptr, nullptr,
                            instance_, this);
    return hwnd_ != nullptr;
  }

  void install_renderer_bridge() {
    static constexpr wchar_t kBridgeScript[] = LR"JS(
(() => {
  if (window.ecnuVpn || !window.chrome || !window.chrome.webview) return;
  let nextId = 1;
  const pending = new Map();
  const subscribers = new Set();
  function rpc(action, payload) {
    const id = nextId++;
    const request = { id, action, payload: payload ?? {} };
    window.chrome.webview.postMessage(JSON.stringify(request));
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
    });
  }
  function unsupported(name) {
    return Promise.reject(new Error(`${name} is not available in the native WebView shell yet.`));
  }
  window.chrome.webview.addEventListener('message', (event) => {
    const message = event.data;
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
  });
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
    webview_->AddScriptToExecuteOnDocumentCreated(kBridgeScript, nullptr);
  }

  void flush_pending_events() {
    for (const std::string &event_json : pending_events_) {
      emit_event(event_json);
    }
    pending_events_.clear();
  }

  void fail_and_close(const wchar_t *message) {
    MessageBoxW(hwnd_, message, L"ECNU VPN", MB_ICONERROR | MB_OK);
    exit_code_ = 70;
    running_ = false;
    if (hwnd_) {
      DestroyWindow(hwnd_);
    }
  }

  static LRESULT CALLBACK window_proc(HWND hwnd, UINT message, WPARAM wparam,
                                      LPARAM lparam) {
    WebView2Window *self =
        reinterpret_cast<WebView2Window *>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
    if (message == WM_NCCREATE) {
      auto *create = reinterpret_cast<CREATESTRUCTW *>(lparam);
      self = reinterpret_cast<WebView2Window *>(create->lpCreateParams);
      SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(self));
      self->hwnd_ = hwnd;
    }

    if (self) {
      switch (message) {
      case WM_SIZE:
        self->resize_webview();
        return 0;
      case WM_CLOSE:
        DestroyWindow(hwnd);
        return 0;
      case WM_DESTROY:
        self->running_ = false;
        return 0;
      default:
        break;
      }
    }
    return DefWindowProcW(hwnd, message, wparam, lparam);
  }

  HINSTANCE instance_ = nullptr;
  HWND hwnd_ = nullptr;
  bool running_ = false;
  int exit_code_ = 70;
  EventRegistrationToken web_message_token_{};
  ecnuvpn::ui_shell::UiWindowConfig active_config_;
  ecnuvpn::ui_shell::HostMessageHandler handler_;
  ComPtr<ICoreWebView2Environment> environment_;
  ComPtr<ICoreWebView2Controller> controller_;
  ComPtr<ICoreWebView2> webview_;
  std::vector<std::string> pending_events_;
};

HRESULT EnvironmentCompletedHandler::QueryInterface(REFIID riid, void **object) {
  if (!object) {
    return E_POINTER;
  }
  if (IsEqualIID(riid, IID_IUnknown) ||
      IsEqualIID(riid, IID_ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler)) {
    *object = static_cast<ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler *>(this);
    AddRef();
    return S_OK;
  }
  *object = nullptr;
  return E_NOINTERFACE;
}

HRESULT EnvironmentCompletedHandler::Invoke(
    HRESULT error_code, ICoreWebView2Environment *result) {
  owner_->on_environment_created(error_code, result);
  return S_OK;
}

HRESULT ControllerCompletedHandler::QueryInterface(REFIID riid, void **object) {
  if (!object) {
    return E_POINTER;
  }
  if (IsEqualIID(riid, IID_IUnknown) ||
      IsEqualIID(riid, IID_ICoreWebView2CreateCoreWebView2ControllerCompletedHandler)) {
    *object = static_cast<ICoreWebView2CreateCoreWebView2ControllerCompletedHandler *>(this);
    AddRef();
    return S_OK;
  }
  *object = nullptr;
  return E_NOINTERFACE;
}

HRESULT ControllerCompletedHandler::Invoke(HRESULT error_code,
                                           ICoreWebView2Controller *result) {
  owner_->on_controller_created(error_code, result);
  return S_OK;
}

HRESULT WebMessageReceivedHandler::QueryInterface(REFIID riid, void **object) {
  if (!object) {
    return E_POINTER;
  }
  if (IsEqualIID(riid, IID_IUnknown) ||
      IsEqualIID(riid, IID_ICoreWebView2WebMessageReceivedEventHandler)) {
    *object = static_cast<ICoreWebView2WebMessageReceivedEventHandler *>(this);
    AddRef();
    return S_OK;
  }
  *object = nullptr;
  return E_NOINTERFACE;
}

HRESULT WebMessageReceivedHandler::Invoke(
    ICoreWebView2 *, ICoreWebView2WebMessageReceivedEventArgs *args) {
  return owner_->on_web_message(args);
}

#else

class WebView2Window final : public ecnuvpn::ui_shell::UiWindow {
public:
  void set_message_handler(ecnuvpn::ui_shell::HostMessageHandler handler) override {
    handler_ = std::move(handler);
  }

  int run(const ecnuvpn::ui_shell::UiWindowConfig &) override {
    return 70;
  }

  void emit_event(const std::string &event_json) override {
    last_event_json_ = event_json;
  }

private:
  ecnuvpn::ui_shell::HostMessageHandler handler_;
  std::string last_event_json_;
};

#endif

} // namespace

ecnuvpn::ui_shell::WindowBounds webview2_default_window_bounds() noexcept {
  return ecnuvpn::ui_shell::kElectronAdvancedWindowBounds;
}

std::wstring webview2_renderer_uri(
    const ecnuvpn::ui_shell::RendererAssets &renderer) {
  if (renderer.kind == ecnuvpn::ui_shell::RendererAssetKind::DevServer) {
    return wide_from_utf8(renderer.location);
  }

  std::filesystem::path path =
      std::filesystem::absolute(std::filesystem::path(renderer.location));
  std::wstring filename = path.filename().generic_wstring();
  const std::string encoded =
      percent_encode_file_uri_path(utf8_from_wide(filename.c_str()));
  if (encoded.empty()) {
    return {};
  }
  return wide_from_utf8(std::string("https://") + kPackagedRendererHost + "/" +
                        encoded);
}

std::wstring webview2_packaged_renderer_folder(
    const ecnuvpn::ui_shell::RendererAssets &renderer) {
  if (renderer.kind == ecnuvpn::ui_shell::RendererAssetKind::DevServer) {
    return {};
  }

  std::filesystem::path path =
      std::filesystem::absolute(std::filesystem::path(renderer.location));
  return path.parent_path().wstring();
}

std::string dispatch_webview2_host_message(
    const std::string &message_json,
    const ecnuvpn::ui_shell::CoreRpcInvoker &invoke_core) {
  return ecnuvpn::ui_shell::handle_host_request(message_json, invoke_core);
}

void post_webview2_host_response(
    const std::string &message_json,
    const ecnuvpn::ui_shell::CoreRpcInvoker &invoke_core,
    const std::function<void(const std::string &)> &post_response) {
  post_response(dispatch_webview2_host_message(message_json, invoke_core));
}

std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_webview2_window() {
  return std::make_unique<WebView2Window>();
}

} // namespace ecnuvpn::platform::win32::ui_shell
