#include "feedback/feedback.hpp"

#include <cassert>
#include <iostream>
#include <string>

using namespace ecnuvpn;

namespace {

int failures = 0;

void expect(bool condition, const std::string &label) {
  if (!condition) {
    std::cerr << "FAIL: " << label << "\n";
    ++failures;
  }
}

// INVARIANT: every failure carries a non-empty canonical code, killing the
// context-free generic popup the user reported.
void test_error_code_is_never_empty() {
  expect(!feedback::resolve_error_code("", "").empty(),
         "empty code+message resolves to non-empty code");
  expect(feedback::resolve_error_code("", "") == feedback::code::kConnectionFailed,
         "empty input collapses to connection_failed");

  auto err = feedback::make_error("Failed to establish the VPN connection.");
  expect(err.value("ok", true) == false, "make_error marks ok=false");
  expect(!err.value("code", std::string()).empty(),
         "make_error always emits a code");
  expect(err.contains("recoverable"), "make_error emits recoverable");
  expect(err.contains("recommended_action"),
         "make_error emits recommended_action");
}

void test_canonical_codes_pass_through() {
  expect(feedback::resolve_error_code("auth_failed", "") ==
             feedback::code::kAuthFailed,
         "auth_failed passes through");
  expect(feedback::resolve_error_code("tls_verify_failed", "") ==
             feedback::code::kTlsVerifyFailed,
         "tls_verify_failed passes through");
  expect(feedback::resolve_error_code("unsupported_auth_flow", "") ==
             feedback::code::kAuthFailed,
         "legacy unsupported_auth_flow maps to auth_failed");
}

void test_message_heuristics() {
  expect(feedback::resolve_error_code("", "Wintun adapter not found") ==
             feedback::code::kWintunMissing,
         "wintun message maps to wintun_missing");
  expect(feedback::resolve_error_code("", "certificate verify failed") ==
             feedback::code::kTlsVerifyFailed,
         "certificate message maps to tls_verify_failed");
  expect(feedback::resolve_error_code("", "用户已取消了操作") ==
             feedback::code::kUserCancelled,
         "cancel message maps to user_cancelled");
}

void test_normalize_error() {
  nlohmann::json raw{{"ok", false}, {"message", "boom"}};
  auto normalized = feedback::normalize_error(raw);
  expect(!normalized.value("code", std::string()).empty(),
         "normalize_error fills missing code");

  nlohmann::json ok{{"ok", true}, {"message", "fine"}};
  auto passthrough = feedback::normalize_error(ok);
  expect(passthrough.value("ok", false) == true,
         "normalize_error leaves success payloads untouched");
}

} // namespace

int main() {
  test_error_code_is_never_empty();
  test_canonical_codes_pass_through();
  test_message_heuristics();
  test_normalize_error();
  if (failures == 0) {
    std::cout << "feedback_test: all assertions passed\n";
    return 0;
  }
  std::cerr << "feedback_test: " << failures << " assertion(s) failed\n";
  return 1;
}
