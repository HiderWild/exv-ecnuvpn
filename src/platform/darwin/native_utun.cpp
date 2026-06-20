#include "platform/darwin/native_utun.hpp"

#if defined(__APPLE__)
#include <net/if.h>
#include <net/if_utun.h>
#include <sys/ioctl.h>
#include <sys/kern_control.h>
#include <sys/socket.h>
#include <sys/sys_domain.h>
#include <unistd.h>
#endif

#include <cerrno>
#include <cstring>
#include <utility>

namespace exv {
namespace platform {
namespace {

NativeUtunStartResult failure(NativeUtunError error,
                              std::string detail = {},
                              int system_error = 0) {
  NativeUtunStartResult result;
  result.error = error;
  result.detail = std::move(detail);
  result.system_error = system_error;
  return result;
}

bool has_required_api(const NativeUtunApi &api) {
  return static_cast<bool>(api.open_socket) &&
         static_cast<bool>(api.resolve_control_id) &&
         static_cast<bool>(api.connect_utun) &&
         static_cast<bool>(api.get_interface_name) &&
         static_cast<bool>(api.set_mtu) && static_cast<bool>(api.close_fd);
}

int last_error(const NativeUtunApi &api) {
  if (!api.last_error)
    return 0;
  return api.last_error();
}

bool is_permission_error(int system_error) {
  return system_error == EACCES || system_error == EPERM;
}

NativeUtunStartResult failure_from_system_error(NativeUtunError fallback,
                                                int system_error,
                                                const std::string &detail) {
  if (is_permission_error(system_error))
    return failure(NativeUtunError::utun_permission_denied, detail,
                   system_error);
  return failure(fallback, detail, system_error);
}

void close_fd(const NativeUtunApi &api, int fd) {
  if (fd >= 0 && api.close_fd)
    api.close_fd(fd);
}

} // namespace

const char *native_utun_error_code(NativeUtunError error) {
  switch (error) {
  case NativeUtunError::none:
    return "none";
  case NativeUtunError::api_missing:
    return "api_missing";
  case NativeUtunError::invalid_mtu:
    return "invalid_mtu";
  case NativeUtunError::socket_open_failed:
    return "socket_open_failed";
  case NativeUtunError::control_lookup_failed:
    return "control_lookup_failed";
  case NativeUtunError::utun_permission_denied:
    return "utun_permission_denied";
  case NativeUtunError::connect_failed:
    return "connect_failed";
  case NativeUtunError::ifname_query_failed:
    return "ifname_query_failed";
  case NativeUtunError::mtu_set_failed:
    return "mtu_set_failed";
  }
  return "unknown";
}

int native_utun_pf_system() {
#if defined(__APPLE__)
  return PF_SYSTEM;
#else
  return 0;
#endif
}

int native_utun_socket_type() {
#if defined(SOCK_DGRAM)
  return SOCK_DGRAM;
#else
  return 2;
#endif
}

int native_utun_sysproto_control() {
#if defined(__APPLE__)
  return SYSPROTO_CONTROL;
#else
  return 0;
#endif
}

const char *native_utun_control_name() {
#if defined(__APPLE__)
  return UTUN_CONTROL_NAME;
#else
  return "com.apple.net.utun_control";
#endif
}

NativeUtunApi default_native_utun_api() {
  NativeUtunApi api;
#if defined(__APPLE__)
  api.open_socket = [](int domain, int type, int protocol) {
    return ::socket(domain, type, protocol);
  };
  api.resolve_control_id = [](int fd, const std::string &control_name,
                              std::uint32_t &control_id) {
    ctl_info info{};
    std::strncpy(info.ctl_name, control_name.c_str(),
                 sizeof(info.ctl_name) - 1);
    if (::ioctl(fd, CTLIOCGINFO, &info) != 0)
      return -1;
    control_id = info.ctl_id;
    return 0;
  };
  api.connect_utun = [](int fd, std::uint32_t control_id, std::uint32_t unit) {
    sockaddr_ctl address{};
    address.sc_len = sizeof(address);
    address.sc_family = AF_SYSTEM;
    address.ss_sysaddr = AF_SYS_CONTROL;
    address.sc_id = control_id;
    address.sc_unit = unit;
    return ::connect(fd, reinterpret_cast<sockaddr *>(&address),
                     sizeof(address));
  };
  api.get_interface_name = [](int fd, std::string &interface_name) {
    char ifname[IFNAMSIZ] = {0};
    socklen_t length = sizeof(ifname);
    if (::getsockopt(fd, SYSPROTO_CONTROL, UTUN_OPT_IFNAME, ifname,
                     &length) != 0)
      return -1;
    ifname[IFNAMSIZ - 1] = '\0';
    interface_name = ifname;
    return interface_name.empty() ? -1 : 0;
  };
  api.set_mtu = [](const std::string &interface_name, int mtu) {
    int socket_fd = ::socket(AF_INET, SOCK_DGRAM, 0);
    if (socket_fd < 0)
      return -1;

    ifreq request{};
    std::strncpy(request.ifr_name, interface_name.c_str(),
                 sizeof(request.ifr_name) - 1);
    request.ifr_mtu = mtu;

    const int result = ::ioctl(socket_fd, SIOCSIFMTU, &request);
    const int saved_errno = errno;
    ::close(socket_fd);
    if (result != 0)
      errno = saved_errno;
    return result;
  };
  api.close_fd = [](int fd) { return ::close(fd); };
  api.last_error = [] { return errno; };
#else
  api.open_socket = [](int, int, int) { return -1; };
  api.resolve_control_id = [](int, const std::string &, std::uint32_t &) {
    return -1;
  };
  api.connect_utun = [](int, std::uint32_t, std::uint32_t) { return -1; };
  api.get_interface_name = [](int, std::string &) { return -1; };
  api.set_mtu = [](const std::string &, int) { return -1; };
  api.close_fd = [](int) { return 0; };
  api.last_error = [] { return 0; };
#endif
  return api;
}

NativeUtun::NativeUtun(NativeUtunApi api, NativeUtunConfig config)
    : api_(std::move(api)), config_(config) {}

NativeUtun::~NativeUtun() { stop(); }

NativeUtunStartResult NativeUtun::start() {
  NativeUtunStartResult result;
  if (fd_ >= 0) {
    result.metadata = metadata_;
    return result;
  }

  if (!has_required_api(api_))
    return failure(NativeUtunError::api_missing,
                   "native utun API table is incomplete");

  if (config_.mtu <= 0)
    return failure(NativeUtunError::invalid_mtu,
                   "utun MTU must be greater than zero");

  int fd = api_.open_socket(native_utun_pf_system(), native_utun_socket_type(),
                            native_utun_sysproto_control());
  if (fd < 0) {
    return failure_from_system_error(
        NativeUtunError::socket_open_failed, last_error(api_),
        "failed to open utun control socket");
  }

  std::uint32_t control_id = 0;
  if (api_.resolve_control_id(fd, native_utun_control_name(), control_id) != 0) {
    const int system_error = last_error(api_);
    close_fd(api_, fd);
    return failure_from_system_error(
        NativeUtunError::control_lookup_failed, system_error,
        "failed to resolve utun control id");
  }

  if (api_.connect_utun(fd, control_id, config_.unit) != 0) {
    const int system_error = last_error(api_);
    close_fd(api_, fd);
    return failure_from_system_error(NativeUtunError::connect_failed,
                                     system_error,
                                     "failed to connect utun control socket");
  }

  std::string interface_name;
  if (api_.get_interface_name(fd, interface_name) != 0 ||
      interface_name.empty()) {
    const int system_error = last_error(api_);
    close_fd(api_, fd);
    return failure_from_system_error(NativeUtunError::ifname_query_failed,
                                     system_error,
                                     "failed to query utun interface name");
  }

  if (api_.set_mtu(interface_name, config_.mtu) != 0) {
    const int system_error = last_error(api_);
    close_fd(api_, fd);
    return failure_from_system_error(NativeUtunError::mtu_set_failed,
                                     system_error,
                                     "failed to set utun interface MTU");
  }

  fd_ = fd;
  metadata_.fd = fd;
  metadata_.interface_name = interface_name;
  metadata_.mtu = config_.mtu;

  result.metadata = metadata_;
  return result;
}

void NativeUtun::stop() {
  if (fd_ >= 0)
    close_fd(api_, fd_);
  fd_ = -1;
  metadata_ = {};
}

bool NativeUtun::running() const { return fd_ >= 0; }

const NativeUtunMetadata &NativeUtun::metadata() const { return metadata_; }

} // namespace platform
} // namespace exv
