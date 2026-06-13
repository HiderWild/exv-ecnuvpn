#include "core/config/config.hpp"
#include "crypto.hpp"
#include "logger.hpp"
#include "utils.hpp"

#include <iostream>

namespace ecnuvpn {
namespace config {

// ── Key management ──────────────────────────────────────────────

void key_show() {
  utils::print_header("Encryption Key Status");
  std::string ks = crypto::key_status();
  std::cout << "  Key file : " << crypto::key_path() << std::endl;
  std::cout << "  Status   : ";
  if (ks == "valid")
    std::cout << utils::GREEN << utils::BOLD << "valid" << utils::RESET
              << std::endl;
  else if (ks == "missing") {
    std::cout << utils::YELLOW << utils::BOLD << "missing" << utils::RESET
              << std::endl;
    utils::print_info("Run: exv config key reset");
  } else {
    std::cout << utils::RED << utils::BOLD << "corrupt" << utils::RESET
              << std::endl;
    utils::print_warning("Run: exv config key reset");
  }
  std::cout << std::endl;
}

bool key_reset() { return crypto::reset_key(); }

// ── get_plaintext_password ──────────────────────────────────────

std::string get_plaintext_password(const Config &cfg) {
  if (cfg.password.empty())
    return "";

  std::string ks = crypto::key_status();
  if (ks != "valid") {
    utils::print_warning("Encryption key is " + ks +
                         ". Cannot decrypt stored password.");
    logger::error("Cannot decrypt password: key status is " + ks);
    return "";
  }

  std::string plaintext = crypto::decrypt(cfg.password, crypto::load_key());
  if (plaintext.empty() && !cfg.password.empty()) {
    utils::print_warning("Failed to decrypt stored password.");
    utils::print_info(
        "The encryption key may have changed. Run 'exv config key reset' then re-set password.");
    logger::error("Password decryption returned empty");
  }
  return plaintext;
}

} // namespace config
} // namespace ecnuvpn
