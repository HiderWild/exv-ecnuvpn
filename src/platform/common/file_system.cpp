#include "platform/common/file_system.hpp"

#include "platform/common/runtime_paths.hpp"

#include <fstream>
#include <sstream>
#ifdef _WIN32
#include <direct.h>
#include <sys/stat.h>
#else
#include <sys/stat.h>
#endif

namespace exv::platform {

bool file_exists(const std::string &path) {
#ifndef _WIN32
  struct stat st;
  return stat(path.c_str(), &st) == 0;
#else
  struct _stat st;
  return _stat(path.c_str(), &st) == 0;
#endif
}

bool ensure_dir(const std::string &path) {
#ifndef _WIN32
  struct stat st;
  if (stat(path.c_str(), &st) == 0) {
    return S_ISDIR(st.st_mode) && sync_owner(path);
  }
  return mkdir(path.c_str(), 0755) == 0 && sync_owner(path);
#else
  struct _stat st;
  if (_stat(path.c_str(), &st) == 0) {
    return (st.st_mode & _S_IFDIR) != 0 && sync_owner(path);
  }
  return _mkdir(path.c_str()) == 0 && sync_owner(path);
#endif
}

std::string read_file(const std::string &path) {
  std::ifstream ifs(path);
  if (!ifs.is_open()) {
    return "";
  }
  std::ostringstream ss;
  ss << ifs.rdbuf();
  return ss.str();
}

bool write_file(const std::string &path, const std::string &content) {
  std::ofstream ofs(path);
  if (!ofs.is_open()) {
    return false;
  }
  ofs << content;
  return ofs.good() && sync_owner(path);
}

} // namespace exv::platform

