#pragma once

#include <cstdint>
#include <functional>
#include <string>

namespace exv {
namespace platform {

enum class NativeUtunError {
  none,
  api_missing,
  invalid_mtu,
  socket_open_failed,
  control_lookup_failed,
  utun_permission_denied,
  connect_failed,
  ifname_query_failed,
  mtu_set_failed,
};

const char *native_utun_error_code(NativeUtunError error);

struct NativeUtunMetadata {
  int fd = -1;
  std::string interface_name;
  int mtu = 0;
};

struct NativeUtunStartResult {
  NativeUtunError error = NativeUtunError::none;
  NativeUtunMetadata metadata;
  std::string detail;
  int system_error = 0;

  bool ok() const { return error == NativeUtunError::none; }
};

struct NativeUtunApi {
  std::function<int(int, int, int)> open_socket;
  std::function<int(int, const std::string &, std::uint32_t &)>
      resolve_control_id;
  std::function<int(int, std::uint32_t, std::uint32_t)> connect_utun;
  std::function<int(int, std::string &)> get_interface_name;
  std::function<int(const std::string &, int)> set_mtu;
  std::function<int(int)> close_fd;
  std::function<int()> last_error;
};

NativeUtunApi default_native_utun_api();

struct NativeUtunConfig {
  int mtu = 1290;
  std::uint32_t unit = 0;
};

class NativeUtun {
public:
  explicit NativeUtun(NativeUtunApi api = default_native_utun_api(),
                      NativeUtunConfig config = {});
  ~NativeUtun();

  NativeUtun(const NativeUtun &) = delete;
  NativeUtun &operator=(const NativeUtun &) = delete;

  NativeUtunStartResult start();
  void stop();

  bool running() const;
  const NativeUtunMetadata &metadata() const;

private:
  NativeUtunApi api_;
  NativeUtunConfig config_;
  int fd_ = -1;
  NativeUtunMetadata metadata_;
};

int native_utun_pf_system();
int native_utun_socket_type();
int native_utun_sysproto_control();
const char *native_utun_control_name();

} // namespace platform
} // namespace exv
