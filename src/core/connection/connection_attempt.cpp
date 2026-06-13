#include "connection_attempt.hpp"

#include <atomic>
#include <chrono>
#include <cerrno>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <system_error>
#include <utility>

#ifdef _WIN32
#include <windows.h>
#else
#include <fcntl.h>
#include <signal.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>
#endif

namespace ecnuvpn {
namespace connection_attempt {

#include "core/connection/connection_attempt_internal.inc.cpp"
#include "core/connection/connection_attempt_public.inc.cpp"

} // namespace connection_attempt
} // namespace ecnuvpn
