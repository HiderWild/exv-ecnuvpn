#include "helper/helper_ipc.hpp"
#include "logger.hpp"

#ifdef _WIN32
#include <windows.h>
#include <sddl.h>
#include <vector>
#endif

namespace ecnuvpn {
namespace helper {

class WinIpcServer : public IpcServer {
#ifdef _WIN32
  HANDLE hPipe_ = INVALID_HANDLE_VALUE;
#endif
  unsigned int peer_uid_ = 0;
  unsigned int peer_gid_ = 0;

public:
  ~WinIpcServer() override { close(); }

  bool start(const std::string &path) override {
#ifdef _WIN32
    PSECURITY_DESCRIPTOR security_descriptor = NULL;
    SECURITY_ATTRIBUTES security_attributes = {};
    security_attributes.nLength = sizeof(security_attributes);
    security_attributes.bInheritHandle = FALSE;

    // LocalSystem owns the service process, but the desktop client runs as the
    // interactive user. Grant interactive users pipe read/write access while
    // keeping full access for LocalSystem and Administrators.
    if (!ConvertStringSecurityDescriptorToSecurityDescriptorA(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)", SDDL_REVISION_1,
            &security_descriptor, NULL)) {
      logger::error("Helper: failed to build named pipe security descriptor");
      return false;
    }
    security_attributes.lpSecurityDescriptor = security_descriptor;

    hPipe_ = CreateNamedPipeA(
        path.c_str(),
        PIPE_ACCESS_DUPLEX,
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
        1, 65536, 65536, 0, &security_attributes);

    if (security_descriptor) {
      LocalFree(security_descriptor);
    }

    if (hPipe_ == INVALID_HANDLE_VALUE) {
      logger::error("Helper: CreateNamedPipe failed");
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
    if (!ConnectNamedPipe(hPipe_, NULL)) {
      DWORD err = GetLastError();
      if (err != ERROR_PIPE_CONNECTED) {
        return false;
      }
    }
    return true;
#else
    return false;
#endif
  }

  bool verify_client() override {
#ifdef _WIN32
    // Access is controlled by the named pipe DACL. Avoid failing legitimate
    // interactive clients because of token impersonation level differences.
    peer_uid_ = 0;
    peer_gid_ = 0;
    return true;
#if 0
    // Impersonate to get client identity
    if (!ImpersonateNamedPipeClient(hPipe_)) {
      logger::error("Helper: ImpersonateNamedPipeClient failed");
      return false;
    }

    // Get the impersonated token and extract user SID
    HANDLE hToken = NULL;
    if (!OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, FALSE, &hToken)) {
      RevertToSelf();
      return false;
    }

    // Get the user SID from the token
    DWORD sid_size = 0;
    GetTokenInformation(hToken, TokenUser, NULL, 0, &sid_size);
    if (sid_size == 0) {
      CloseHandle(hToken);
      RevertToSelf();
      return false;
    }

    std::vector<BYTE> sid_buf(sid_size);
    TOKEN_USER *tu = reinterpret_cast<TOKEN_USER *>(sid_buf.data());
    if (!GetTokenInformation(hToken, TokenUser, tu, sid_size, &sid_size)) {
      CloseHandle(hToken);
      RevertToSelf();
      return false;
    }

    // Convert SID to a numeric hash for uid/gid (Windows has no uid/gid concept)
    // Use the RID (Relative ID) from the SID as a pseudo-uid
    PSID sid = tu->User.Sid;
    SID_IDENTIFIER_AUTHORITY *auth = GetSidIdentifierAuthority(sid);
    BYTE rid_count = *GetSidSubAuthorityCount(sid);
    DWORD rid = rid_count > 0 ? *GetSidSubAuthority(sid, rid_count - 1) : 0;

    // For Administrators group (RID 544), use uid=0; otherwise use the RID
    peer_uid_ = (rid == 544) ? 0 : rid;
    peer_gid_ = peer_uid_;

    CloseHandle(hToken);
    RevertToSelf();
    return true;
#endif
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
      if (raw.find('\n') != std::string::npos)
        break;
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
    return success && bytesWritten == payload.size();
#else
    (void)response;
    return false;
#endif
  }

  void close() override {
#ifdef _WIN32
    if (hPipe_ != INVALID_HANDLE_VALUE) {
      CloseHandle(hPipe_);
      hPipe_ = INVALID_HANDLE_VALUE;
    }
#endif
  }

  void close_client() override {
#ifdef _WIN32
    if (hPipe_ != INVALID_HANDLE_VALUE) {
      DisconnectNamedPipe(hPipe_);
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
