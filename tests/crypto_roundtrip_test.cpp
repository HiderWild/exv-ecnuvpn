#include "core/crypto/crypto.hpp"
#include "common/diagnostics/logger.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <string>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

std::filesystem::path temp_root;

std::string trim_copy(const std::string &value) {
  const auto first = value.find_first_not_of(" \t\r\n");
  if (first == std::string::npos)
    return "";
  const auto last = value.find_last_not_of(" \t\r\n");
  return value.substr(first, last - first + 1);
}

} // namespace

namespace ecnuvpn {
namespace logger {

void init() {}
void info(const std::string &) {}
void error(const std::string &message) { std::cerr << message << std::endl; }
void warn(const std::string &) {}
void show_logs(int) {}

} // namespace logger

namespace platform {

std::string expand_home(const std::string &path) { return path; }
std::string get_redirect_path() { return ""; }
std::string get_config_dir() { return temp_root.string(); }
bool set_config_dir(const std::string &) { return true; }
std::string get_config_path() { return (temp_root / "config.json").string(); }
std::string get_pid_path() { return ""; }
std::string get_log_path() { return ""; }
std::string get_tunnel_path() { return ""; }
std::string get_supervisor_pid_path() { return ""; }
std::string get_route_ready_path() { return ""; }
std::string get_effective_home() { return temp_root.string(); }
std::string get_home_for_uid(unsigned int) { return temp_root.string(); }
std::string get_username_for_uid(unsigned int) { return ""; }
std::string get_config_dir_for_uid(unsigned int) { return temp_root.string(); }
void set_runtime_path_override(const std::string &, const std::string &) {}
void clear_runtime_path_override() {}
void set_runtime_owner(unsigned int, unsigned int) {}
void clear_runtime_owner() {}
bool has_runtime_owner() { return false; }
unsigned int get_runtime_owner_uid() { return 0; }
unsigned int get_runtime_owner_gid() { return 0; }
bool sync_owner(const std::string &) { return true; }
std::string get_executable_path() { return ""; }

bool fix_runtime_config_dir_ownership() { return true; }

bool file_exists(const std::string &path) {
  return std::filesystem::exists(path);
}
bool ensure_dir(const std::string &path) {
  std::error_code ec;
  std::filesystem::create_directories(path, ec);
  return !ec && std::filesystem::exists(path);
}
std::string read_file(const std::string &path) {
  std::ifstream ifs(path, std::ios::binary);
  return std::string(std::istreambuf_iterator<char>(ifs),
                     std::istreambuf_iterator<char>());
}
bool write_file(const std::string &path, const std::string &content) {
  std::ofstream ofs(path, std::ios::binary);
  ofs << content;
  return ofs.good();
}

std::string get_bundled_runtime_dir() { return ""; }
std::string get_bundled_openconnect_path() { return ""; }
std::string get_bundled_wintun_path() { return ""; }
std::string get_bundled_tap_installer_path() { return ""; }
std::string get_openconnect_path(const std::string &) { return ""; }
bool check_openconnect(const std::string &) { return true; }
bool check_root() { return false; }
bool get_interface_traffic(const std::string &, uint64_t *, uint64_t *) {
  return false;
}
int run_command(const std::string &) { return 0; }
std::string run_command_output(const std::string &) { return ""; }
std::string shell_quote(const std::string &value) { return value; }

} // namespace platform
} // namespace ecnuvpn

int main() {
  bool ok = true;

  temp_root = std::filesystem::temp_directory_path() /
              std::filesystem::path("ecnuvpn-crypto-roundtrip-test");
  std::error_code ec;
  std::filesystem::remove_all(temp_root, ec);
  std::filesystem::create_directories(temp_root, ec);

  std::string generated_key = ecnuvpn::crypto::generate_key();
  ok = expect(ecnuvpn::crypto::validate_key(generated_key),
              "generate_key should return a valid 64-character hex key") &&
       ok;

  ok = expect(ecnuvpn::crypto::save_key(generated_key),
              "save_key should persist a generated key") &&
       ok;
  ok = expect(ecnuvpn::crypto::load_key() == generated_key,
              "load_key should roundtrip the saved key") &&
       ok;

  std::string ciphertext =
      ecnuvpn::crypto::encrypt("vpn-secret", generated_key);
  ok = expect(!ciphertext.empty(),
              "encrypt should produce ciphertext for a valid key") &&
       ok;
  std::string decrypted = ecnuvpn::crypto::decrypt(ciphertext, generated_key);
  if (decrypted != "vpn-secret") {
    std::cerr << "decrypt mismatch: [" << decrypted << "] len="
              << decrypted.size() << std::endl;
  }
  ok = expect(decrypted == "vpn-secret",
              "decrypt should recover the original plaintext") &&
       ok;

  std::filesystem::remove(ecnuvpn::crypto::key_path(), ec);
  std::string initialized_key = ecnuvpn::crypto::init_key_if_needed();
  ok = expect(ecnuvpn::crypto::validate_key(initialized_key),
              "init_key_if_needed should recreate a valid key when missing") &&
       ok;
  ok = expect(std::filesystem::exists(ecnuvpn::crypto::key_path()),
              "init_key_if_needed should recreate the key file") &&
       ok;

  std::filesystem::remove_all(temp_root, ec);
  return ok ? 0 : 1;
}
