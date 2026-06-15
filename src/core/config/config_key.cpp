#include "core/config/config.hpp"
#include "core/crypto/crypto.hpp"
#include "common/diagnostics/logger.hpp"
#include "cli/console.hpp"

#include <iostream>

namespace ecnuvpn {
namespace config {

// ── Key management ──────────────────────────────────────────────

void key_show() {
  cli::print_header("Encryption Key Status");
  std::string ks = crypto::key_status();
  std::cout << "  Key file : " << crypto::key_path() << std::endl;
  std::cout << "  Status   : ";
  if (ks == "valid")
    std::cout << cli::GREEN << cli::BOLD << "valid" << cli::RESET
              << std::endl;
  else if (ks == "missing") {
    std::cout << cli::YELLOW << cli::BOLD << "missing" << cli::RESET
              << std::endl;
    cli::print_info("Run: exv config key reset");
  } else {
    std::cout << cli::RED << cli::BOLD << "corrupt" << cli::RESET
              << std::endl;
    cli::print_warning("Run: exv config key reset");
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
    cli::print_warning("Encryption key is " + ks +
                         ". Cannot decrypt stored password.");
    logger::error("Cannot decrypt password: key status is " + ks);
    return "";
  }

  std::string plaintext = crypto::decrypt(cfg.password, crypto::load_key());
  if (plaintext.empty() && !cfg.password.empty()) {
    cli::print_warning("Failed to decrypt stored password.");
    cli::print_info(
        "The encryption key may have changed. Run 'exv config key reset' then re-set password.");
    logger::error("Password decryption returned empty");
  }
  return plaintext;
}

} // namespace config
} // namespace ecnuvpn
