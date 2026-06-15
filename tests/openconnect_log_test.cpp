#include "vpn_engine/openconnect/openconnect_log.hpp"

#include <iostream>
#include <string>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

} // namespace

int main() {
  bool ok = true;

  auto failed = ecnuvpn::openconnect_log::parse_evidence(
      "Starting VPN: vpn.example.edu user=test\n"
      "POST https://vpn.example.edu/\n"
      "Authentication failed\n");
  ok = expect(failed.auth_failed,
              "authentication failed should be detected in latest attempt") &&
       ok;
  ok = expect(!failed.has_tunnel_metadata,
              "failed attempt should not invent tunnel metadata") &&
       ok;
  ok = expect(ecnuvpn::openconnect_log::contains_auth_failure_text(
                  "GROUP: [ECNU]:ECNU\nLogin failed.\nPassword:"),
              "incremental log hook should detect login failed text") &&
       ok;
  ok = expect(ecnuvpn::openconnect_log::contains_auth_failure_text(
                  "XML response has no \"auth\" node\n"
                  "Failed to complete authentication\n"),
              "incremental log hook should detect auth form failures") &&
       ok;

  auto stale_success_then_failed = ecnuvpn::openconnect_log::parse_evidence(
      "Starting VPN: vpn.example.edu user=test\n"
      "Using Wintun device 'ECNUVPN-OLD', index 31\n"
      "Configured as 10.8.0.2, with SSL connected\n"
      "Starting VPN: vpn.example.edu user=test\n"
      "Login failed\n");
  ok = expect(stale_success_then_failed.auth_failed,
              "latest failed attempt should be reported as auth failure") &&
       ok;
  ok = expect(!stale_success_then_failed.has_tunnel_metadata,
              "old successful tunnel metadata must not leak into latest attempt") &&
       ok;

  auto success = ecnuvpn::openconnect_log::parse_evidence(
      "Starting VPN: vpn.example.edu user=test\n"
      "Using Wintun device 'ECNUVPN-ABCD', index 42\n"
      "Configured as 10.42.0.10, with SSL connected\n");
  ok = expect(!success.auth_failed,
              "successful attempt should not be marked as auth failure") &&
       ok;
  ok = expect(success.has_tunnel_metadata,
              "successful attempt should expose tunnel metadata") &&
       ok;
  ok = expect(success.internal_ip == "10.42.0.10",
              "successful attempt should parse internal IP") &&
       ok;
  ok = expect(success.adapter == "ECNUVPN-ABCD",
              "successful attempt should parse adapter name") &&
       ok;
  ok = expect(success.if_index == "42",
              "successful attempt should parse interface index") &&
       ok;

  return ok ? 0 : 1;
}
