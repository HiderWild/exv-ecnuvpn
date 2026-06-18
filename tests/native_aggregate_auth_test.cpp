#include "vpn_engine/protocol/aggregate_auth.hpp"

#include <iostream>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

ecnuvpn::vpn_engine::protocol::HttpResponse response(
    int status, std::string body,
    std::vector<std::string> set_cookies = {}) {
  ecnuvpn::vpn_engine::protocol::HttpResponse out;
  out.status = status;
  out.body = std::move(body);
  if (!set_cookies.empty()) {
    out.header_values["set-cookie"] = set_cookies;
    std::string flattened;
    for (const auto &cookie : set_cookies) {
      if (!flattened.empty())
        flattened += ", ";
      flattened += cookie;
    }
    out.headers["set-cookie"] = flattened;
  }
  return out;
}

} // namespace

int main() {
  bool ok = true;

  {
    const std::string xml =
        ecnuvpn::vpn_engine::protocol::make_aggregate_auth_reply_xml(
            "alice&bob", "p<ass&word", "students", "123456");
    ok = expect(xml.find("<config-auth client=\"vpn\" type=\"auth-reply\">") !=
                    std::string::npos,
                "auth reply should use aggregate-auth root") &&
         ok;
    ok = expect(xml.find("<username>alice&amp;bob</username>") !=
                    std::string::npos,
                "username should be XML-escaped") &&
         ok;
    ok = expect(xml.find("<password>p&lt;ass&amp;word</password>") !=
                    std::string::npos,
                "password should be XML-escaped") &&
         ok;
    ok = expect(xml.find("<secondary_password>123456</secondary_password>") !=
                    std::string::npos,
                "secondary password should be included when provided") &&
         ok;
  }

  {
    auto parsed = ecnuvpn::vpn_engine::protocol::parse_aggregate_auth_response(
        response(200,
                 "<config-auth><session-token>SESSION&amp;TOKEN</session-token>"
                 "</config-auth>"));
    ok = expect(parsed.ok, "session-token response should parse") && ok;
    ok = expect(parsed.cookie == "webvpn=SESSION&TOKEN",
                "session-token should map to webvpn cookie") &&
         ok;
  }

  {
    auto parsed = ecnuvpn::vpn_engine::protocol::parse_aggregate_auth_response(
        response(200, "<config-auth/>",
                 {"webvpn=COOKIE_FROM_HEADER; Path=/; Secure"}));
    ok = expect(parsed.ok, "webvpn Set-Cookie should parse") && ok;
    ok = expect(parsed.cookie == "webvpn=COOKIE_FROM_HEADER",
                "Set-Cookie should map to cookie pair") &&
         ok;
  }

  {
    auto parsed = ecnuvpn::vpn_engine::protocol::parse_aggregate_auth_response(
        response(200,
                 "<config-auth><host-scan><ticket>SECRET-TICKET</ticket>"
                 "</host-scan></config-auth>"));
    ok = expect(!parsed.ok && parsed.error_code == "csd_required_unsupported",
                "host-scan should be explicitly unsupported") &&
         ok;
    ok = expect(parsed.error_message.find("SECRET-TICKET") == std::string::npos,
                "host-scan error should not include ticket values") &&
         ok;
  }

  {
    auto parsed = ecnuvpn::vpn_engine::protocol::parse_aggregate_auth_response(
        response(200,
                 "<config-auth><auth><select name=\"group\">"
                 "<option value=\"students\">Students</option>"
                 "<option value=\"staff\">Staff</option>"
                 "</select></auth></config-auth>"));
    ok = expect(!parsed.ok && parsed.error_code == "auth_group_required",
                "group selection should be reported as continuation") &&
         ok;
    ok = expect(parsed.prompt.kind == "group",
                "group prompt should expose group kind") &&
         ok;
    ok = expect(parsed.prompt.options.size() == 2 &&
                    parsed.prompt.options[0] == "students" &&
                    parsed.prompt.options[1] == "staff",
                "group prompt should parse option values") &&
         ok;
  }

  {
    const std::string xml =
        ecnuvpn::vpn_engine::protocol::make_aggregate_auth_reply_xml(
            "student", "primary", "staff", "654321");
    ok = expect(xml.find("<group>staff</group>") != std::string::npos,
                "follow-up auth reply should preserve selected group") &&
         ok;
    ok = expect(xml.find("<secondary_password>654321</secondary_password>") !=
                    std::string::npos,
                "follow-up auth reply should include challenge response") &&
         ok;
  }

  {
    auto parsed = ecnuvpn::vpn_engine::protocol::parse_aggregate_auth_response(
        response(200,
                 "<config-auth><auth>"
                 "<input name=\"secondary_password\" type=\"password\" "
                 "label=\"Duo passcode\"/>"
                 "</auth></config-auth>"));
    ok = expect(!parsed.ok && parsed.error_code == "auth_challenge_required",
                "password-plus-token form should be reported as challenge") &&
         ok;
    ok = expect(parsed.prompt.kind == "challenge",
                "token prompt should expose challenge kind") &&
         ok;
    ok = expect(parsed.prompt.label == "Duo passcode",
                "token prompt should use structured input label") &&
         ok;
    ok = expect(parsed.prompt.input_type == "password",
                "token prompt should use structured input type") &&
         ok;
  }

  {
    auto parsed = ecnuvpn::vpn_engine::protocol::parse_aggregate_auth_response(
        response(200,
                 "<config-auth><auth>"
                 "<input name=\"answer\" type=\"text\" "
                 "label=\"Security answer\"/>"
                 "</auth></config-auth>"));
    ok = expect(!parsed.ok && parsed.error_code == "auth_challenge_required",
                "informational text prompt should be reported as challenge") &&
         ok;
    ok = expect(parsed.prompt.label == "Security answer",
                "text prompt should use structured input label") &&
         ok;
    ok = expect(parsed.prompt.input_type == "text",
                "text prompt should use structured input type") &&
         ok;
  }

  {
    auto parsed = ecnuvpn::vpn_engine::protocol::parse_aggregate_auth_response(
        response(200,
                 "<config-auth><saml-auth "
                 "url=\"https://idp.example.invalid/sso?SAMLRequest=SECRET\"/>"
                 "</config-auth>"));
    ok = expect(!parsed.ok &&
                    parsed.error_code == "saml_required_unsupported",
                "SAML auth should use public SAML diagnostic code") &&
         ok;
    ok = expect(parsed.error_message.find("idp.example.invalid") ==
                    std::string::npos,
                "SAML error must not include identity provider URL") &&
         ok;
    ok = expect(parsed.error_message.find("SECRET") == std::string::npos,
                "SAML error must not include SAML request secrets") &&
         ok;
  }

  if (ok) {
    std::cout << "native_aggregate_auth_test: all assertions passed\n";
  } else {
    std::cerr << "native_aggregate_auth_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
