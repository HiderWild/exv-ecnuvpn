#include "helper_ipc.hpp"

#ifdef _WIN32
#include <windows.h>
#endif

namespace ecnuvpn {
namespace helper {

class WinIpcServer : public IpcServer {
#ifdef _WIN32
  HANDLE hPipe_ = INVALID_HANDLE_VALUE;
  HANDLE hEvent_ = INVALID_HANDLE_VALUE;
  unsigned int peer_uid_ = 0;
  unsigned int peer_gid_ = 0;
#endif

public:
  ~WinIpcServer() override { close(); }

  bool start(const std::string &path) override {
#ifdef _WIN32
    hPipe_ = CreateNamedPipeA(
        path.c_str(),
        PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
        1, 65536, 65536, 0, NULL);

    if (hPipe_ == INVALID_HANDLE_VALUE) {
      logger::error("Helper: CreateNamedPipe failed");
      return false;
    }

    hEvent_ = CreateEvent(NULL, TRUE, FALSE, NULL);
    if (hEvent_ == INVALID_HANDLE_VALUE) {
      CloseHandle(hPipe_);
      hPipe_ = INVALID_HANDLE_VALUE;
      return false;
    }
    return true;
#else
    (void)path;
    return false;
#endif
  }

  bool accept_client() override {
#ifdef _WIN32
    OVERLAPPED ol = {};
    ol.hEvent = hEvent_;
    ResetEvent(hEvent_);

    ConnectNamedPipe(hPipe_, &ol);

    DWORD waitResult = WaitForSingleObject(hEvent_, INFINITE);
    if (waitResult != WAIT_OBJECT_0) {
      return false;
    }
    return true;
#else
    return false;
#endif
  }

  bool verify_client() override {
#ifdef _WIN32
    // Impersonate to get client identity
    if (!ImpersonateNamedPipeClient(hPipe_)) {
      logger::error("Helper: ImpersonateNamedPipeClient failed");
      return false;
    }

    // Get the impersonated token and extract SID
    HANDLE hToken = NULL;
    if (!OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, FALSE, &hToken)) {
      RevertToSelf();
      return false;
    }

    // For now, allow all local connections
    // TODO: implement proper SID-based access check
    peer_uid_ = 0;
    peer_gid_ = 0;

    CloseHandle(hToken);
    RevertToSelf();
    return true;
#else
    return false;
#endif
  }

  std::string read_request() override {
#ifdef _WIN32
    std::string raw;
    char buffer[4096];
    DWORD bytesRead = 0;

    while (true) {
      BOOL success = ReadFile(hPipe_, buffer, sizeof(buffer) - 1, &bytesRead, NULL);
      if (!success || bytesRead == 0)
        break;
      raw.append(buffer, bytesRead);
    }
    return raw;
#else
    return "";
#endif
  }

  bool send_response(const std::string &response) override {
#ifdef _WIN32
    std::string payload = response;
    payload.push_back('\n');
    DWORD bytesWritten = 0;
    BOOL success = WriteFile(hPipe_, payload.c_str(),
                             static_cast<DWORD>(payload.size()), &bytesWritten, NULL);
    FlushFileBuffers(hPipe_);
    DisconnectNamedPipe(hPipe_);
    return success && bytesWritten == payload.size();
#else
    (void)response;
    return false;
#endif
  }

  void close() override {
#ifdef _WIN32
    if (hEvent_ != INVALID_HANDLE_VALUE) {
      CloseHandle(hEvent_);
      hEvent_ = INVALID_HANDLE_VALUE;
    }
    if (hPipe_ != INVALID_HANDLE_VALUE) {
      CloseHandle(hPipe_);
      hPipe_ = INVALID_HANDLE_VALUE;
    }
#endif
  }

  unsigned int peer_uid() const override { return peer_uid_; }
  unsigned int peer_gid() const override { return peer_gid_; }
};

std::unique_ptr<IpcServer> create_ipc_server() {
  return std::make_unique<WinIpcServer>();
}

} // namespace helper
} // namespace ecnuvpn
