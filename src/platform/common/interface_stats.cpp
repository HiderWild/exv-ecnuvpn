#include "platform/common/interface_stats.hpp"

#include "platform/common/process_utils.hpp"

#include <fstream>
#include <sstream>

namespace exv::platform {

bool get_interface_traffic(const std::string &iface, std::uint64_t *rx_bytes,
                           std::uint64_t *tx_bytes) {
  if (iface.empty() || !rx_bytes || !tx_bytes) {
    return false;
  }
  *rx_bytes = 0;
  *tx_bytes = 0;

#ifdef __APPLE__
  std::string output =
      run_command_output("netstat -b -I " + shell_quote(iface) + " 2>/dev/null");
  std::istringstream stream(output);
  std::string header, data;
  if (!std::getline(stream, header) || !std::getline(stream, data)) {
    return false;
  }

  auto col_pos = [](const std::string &h, const std::string &col) -> size_t {
    size_t pos = h.find(col);
    return (pos == std::string::npos) ? 0 : pos;
  };

  size_t ibytes_pos = col_pos(header, "Ibytes");
  size_t obytes_pos = col_pos(header, "Obytes");
  if (ibytes_pos == 0 || obytes_pos == 0) {
    return false;
  }

  auto field_at = [](const std::string &line, size_t pos) -> std::string {
    size_t start = pos;
    while (start > 0 && line[start - 1] != ' ') {
      start--;
    }
    size_t end = pos;
    while (end < line.size() && line[end] != ' ') {
      end++;
    }
    return line.substr(start, end - start);
  };

  try {
    std::string rx = field_at(data, ibytes_pos);
    std::string tx = field_at(data, obytes_pos);
    if (!rx.empty()) {
      *rx_bytes = std::stoull(rx);
    }
    if (!tx.empty()) {
      *tx_bytes = std::stoull(tx);
    }
    return true;
  } catch (...) {
    return false;
  }
#elif defined(_WIN32)
  (void)iface;
  *rx_bytes = 0;
  *tx_bytes = 0;
  return false;
#else
  auto read_sysfs_counter = [](const std::string &path) -> std::uint64_t {
    std::ifstream ifs(path);
    if (!ifs.is_open()) {
      return 0;
    }
    std::string val;
    std::getline(ifs, val);
    try {
      return std::stoull(val);
    } catch (...) {
      return 0;
    }
  };

  std::string base = "/sys/class/net/" + iface + "/statistics/";
  *rx_bytes = read_sysfs_counter(base + "rx_bytes");
  *tx_bytes = read_sysfs_counter(base + "tx_bytes");
  return (*rx_bytes > 0 || *tx_bytes > 0);
#endif
}

} // namespace exv::platform

