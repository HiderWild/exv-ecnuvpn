#include "vpn_engine/protocol/aggregate_auth.hpp"

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

std::string fixture_text(const std::filesystem::path &relative) {
  std::filesystem::path cursor = std::filesystem::current_path();
  for (int i = 0; i < 8; ++i) {
    const std::filesystem::path candidate = cursor / relative;
    if (std::filesystem::exists(candidate)) {
      std::ifstream in(candidate, std::ios::binary);
      return std::string(std::istreambuf_iterator<char>(in),
                         std::istreambuf_iterator<char>());
    }
    if (!cursor.has_parent_path() || cursor == cursor.parent_path())
      break;
    cursor = cursor.parent_path();
  }
  return {};
}

bool has_field(const ecnuvpn::vpn_engine::protocol::AggregateAuthResponse &r,
               const std::string &name) {
  for (const auto &field : r.fields) {
    if (field.name == name)
      return true;
  }
  return false;
}

const ecnuvpn::vpn_engine::protocol::AggregateAuthField *
field_named(const ecnuvpn::vpn_engine::protocol::AggregateAuthResponse &r,
            const std::string &name) {
  for (const auto &field : r.fields) {
    if (field.name == name)
      return &field;
  }
  return nullptr;
}

bool test_builds_deterministic_init_xml() {
  using namespace ecnuvpn::vpn_engine::protocol;

  AggregateAuthInitRequest request;
  request.server_url = "https://vpn.example.invalid/";
  request.device_id = "ECNUVPN-TEST-DEVICE";
  request.version = "ECNUVPN-NATIVE-TEST";

  const std::string xml = build_aggregate_auth_init_xml(request);

  bool ok = true;
  ok = expect(xml.find("<config-auth client=\"vpn\" type=\"init\" "
                       "aggregate-auth-version=\"2\">") !=
                  std::string::npos,
              "init XML should use AnyConnect aggregate-auth v2 root") &&
       ok;
  ok = expect(xml.find("<version who=\"vpn\">ECNUVPN-NATIVE-TEST</version>") !=
                  std::string::npos,
              "init XML should include deterministic VPN version") &&
       ok;
  ok = expect(xml.find("<device-id>ECNUVPN-TEST-DEVICE</device-id>") !=
                  std::string::npos,
              "init XML should include device id") &&
       ok;
  ok = expect(xml.find("<group-access>https://vpn.example.invalid/</group-access>") !=
                  std::string::npos,
              "init XML should include group-access server URL") &&
       ok;
  return ok;
}

bool test_parses_auth_request_and_echoes_opaque() {
  using namespace ecnuvpn::vpn_engine::protocol;

  AggregateAuthResponse response;
  const auto parsed = parse_aggregate_auth_response(
      fixture_text("tests/fixtures/native_anyconnect_v2/auth_init_response.xml"),
      &response);

  bool ok = true;
  ok = expect(parsed.ok, "auth-request fixture should parse") && ok;
  ok = expect(response.type == AggregateAuthResponseType::auth_request,
              "init fixture should parse as auth_request") &&
       ok;
  ok = expect(has_field(response, "username"),
              "auth-request should expose username field") &&
       ok;
  ok = expect(has_field(response, "password"),
              "auth-request should expose password field") &&
       ok;
  ok = expect(has_field(response, "group_list"),
              "auth-request should expose group field") &&
       ok;
  ok = expect(response.opaque_xml.size() == 1 &&
                  response.opaque_xml[0] == "<opaque>OPAQUE_ONE</opaque>",
              "parser should preserve opaque XML for byte-stable echo") &&
       ok;

  AggregateAuthReplyRequest reply;
  reply.username = "student@example.invalid";
  reply.password = "test-mock-password-placeholder";
  reply.selected_group = "students";
  reply.opaque_xml = response.opaque_xml;

  const std::string xml = build_aggregate_auth_reply_xml(reply);
  ok = expect(xml.find("<config-auth client=\"vpn\" type=\"auth-reply\" "
                       "aggregate-auth-version=\"2\">") !=
                  std::string::npos,
              "auth reply should use aggregate-auth v2 root") &&
       ok;
  ok = expect(xml.find("<opaque>OPAQUE_ONE</opaque>") != std::string::npos,
              "auth reply should echo opaque XML") &&
       ok;
  ok = expect(xml.find("<auth>\n") != std::string::npos,
              "auth reply should use direct auth node") &&
       ok;
  ok = expect(xml.find("<username>student@example.invalid</username>") !=
                  std::string::npos,
              "auth reply should include direct username node") &&
       ok;
  ok = expect(xml.find("<password>test-mock-password-placeholder</password>") !=
                  std::string::npos,
              "auth reply should include direct password node") &&
       ok;
  ok = expect(xml.find("<group-select>students</group-select>") !=
                  std::string::npos,
              "auth reply should include selected group as group-select") &&
       ok;
  ok = expect(xml.find("<form>") == std::string::npos &&
                  xml.find("<input") == std::string::npos,
              "auth reply must not wrap credentials as form inputs") &&
       ok;
  return ok;
}

bool test_parses_success_token_without_formatting_cookie() {
  using namespace ecnuvpn::vpn_engine::protocol;

  AggregateAuthResponse response;
  const auto parsed = parse_aggregate_auth_response(
      fixture_text("tests/fixtures/native_anyconnect_v2/auth_success_response.xml"),
      &response);

  bool ok = true;
  ok = expect(parsed.ok, "success fixture should parse") && ok;
  ok = expect(response.type == AggregateAuthResponseType::success,
              "success fixture should parse as success") &&
       ok;
  ok = expect(response.session_token == "V2_SESSION_TOKEN",
              "success fixture should expose session token") &&
       ok;
  ok = expect(response.session_cookie.empty(),
              "aggregate parser should not format webvpn cookie") &&
       ok;
  return ok;
}

bool test_parses_challenge_and_error_shapes() {
  using namespace ecnuvpn::vpn_engine::protocol;

  AggregateAuthResponse challenge;
  const auto parsed_challenge = parse_aggregate_auth_response(
      fixture_text("tests/fixtures/native_anyconnect_v2/auth_challenge_response.xml"),
      &challenge);

  const std::string error_xml =
      "<config-auth client=\"vpn\" type=\"auth-reply\">"
      "<auth id=\"error\"><message>Invalid credentials</message></auth>"
      "<error>Login failed</error>"
      "</config-auth>";

  AggregateAuthResponse error;
  const auto parsed_error = parse_aggregate_auth_response(error_xml, &error);

  bool ok = true;
  ok = expect(parsed_challenge.ok, "challenge fixture should parse") && ok;
  ok = expect(challenge.type == AggregateAuthResponseType::challenge,
              "secondary password fixture should parse as challenge") &&
       ok;
  ok = expect(has_field(challenge, "secondary_password"),
              "challenge fixture should expose secondary password field") &&
       ok;
  const auto *challenge_field = field_named(challenge, "secondary_password");
  ok = expect(challenge_field && challenge_field->label == "Verification code",
              "challenge fixture should expose prompt label") &&
       ok;
  ok = expect(parsed_error.ok, "error XML should parse as structured response") &&
       ok;
  ok = expect(error.type == AggregateAuthResponseType::error,
              "error XML should map to error response type") &&
       ok;
  ok = expect(error.error_code == "auth_rejected",
              "auth error should map to stable auth_rejected code") &&
       ok;
  return ok;
}

bool test_parses_group_select_options() {
  using namespace ecnuvpn::vpn_engine::protocol;

  const std::string group_xml =
      "<config-auth client=\"vpn\" type=\"auth-request\">"
      "<auth id=\"main\"><message>Select VPN group.</message><form>"
      "<select name=\"group_list\" label=\"VPN group\">"
      "<option value=\"students\">Students</option>"
      "<option value=\"staff\">Faculty and staff</option>"
      "</select>"
      "</form></auth><opaque>OPAQUE_ONE</opaque></config-auth>";

  AggregateAuthResponse group;
  const auto parsed = parse_aggregate_auth_response(group_xml, &group);

  bool ok = true;
  ok = expect(parsed.ok, "group-select XML should parse") && ok;
  ok = expect(group.type == AggregateAuthResponseType::group_select,
              "select-only group response should parse as group_select") &&
       ok;
  const auto *group_field = field_named(group, "group_list");
  ok = expect(group_field != nullptr, "group field should be present") && ok;
  ok = expect(group_field && group_field->label == "VPN group",
              "group field should expose label") &&
       ok;
  ok = expect(group_field && group_field->options.size() == 2,
              "group field should expose visible choices") &&
       ok;
  ok = expect(group_field && group_field->options[0].value == "students" &&
                  group_field->options[0].label == "Students",
              "first group option should preserve value and label") &&
       ok;
  ok = expect(group_field && group_field->options[1].value == "staff" &&
                  group_field->options[1].label == "Faculty and staff",
              "second group option should preserve value and label") &&
       ok;
  return ok;
}

bool test_select_option_without_value_uses_visible_text() {
  using namespace ecnuvpn::vpn_engine::protocol;

  const std::string group_xml =
      "<config-auth client=\"vpn\" type=\"auth-request\" "
      "aggregate-auth-version=\"2\">"
      "<auth id=\"main\"><form>"
      "<input type=\"text\" name=\"username\" label=\"Username:\" />"
      "<input type=\"password\" name=\"password\" label=\"Password:\" />"
      "<select name=\"group_list\" label=\"GROUP:\">"
      "<option selected=\"true\">ECNU</option>"
      "</select>"
      "</form></auth></config-auth>";

  AggregateAuthResponse response;
  const auto parsed = parse_aggregate_auth_response(group_xml, &response);

  const auto *group_field = field_named(response, "group_list");
  bool ok = true;
  ok = expect(parsed.ok, "ECNU group XML should parse") && ok;
  ok = expect(group_field != nullptr, "ECNU group field should be present") && ok;
  ok = expect(group_field && group_field->value == "ECNU",
              "option text should become the selected group when value is absent") &&
       ok;
  ok = expect(group_field && group_field->options.size() == 1,
              "option without value should still be preserved") &&
       ok;
  ok = expect(group_field && group_field->options[0].value == "ECNU" &&
                  group_field->options[0].label == "ECNU",
              "option text should fill both value and label") &&
       ok;
  return ok;
}

bool test_parses_host_scan_metadata() {
  using namespace ecnuvpn::vpn_engine::protocol;

  const std::string host_scan_xml =
      "<config-auth client=\"vpn\" type=\"auth-reply\">"
      "<auth id=\"main\"><message>Host scan required</message></auth>"
      "<host-scan ticket=\"CSD_TICKET_SEED\" token=\"CSD_TOKEN_SEED\" "
      "base-uri=\"/+CSCOE+/sdesktop/\" wait-uri=\"/+CSCOE+/sdesktop/wait.html\" />"
      "<opaque>OPAQUE_ONE</opaque>"
      "</config-auth>";

  AggregateAuthResponse response;
  const auto parsed = parse_aggregate_auth_response(host_scan_xml, &response);

  bool ok = true;
  ok = expect(parsed.ok, "host-scan XML should parse") && ok;
  ok = expect(response.type == AggregateAuthResponseType::host_scan,
              "host-scan XML should map to host_scan response type") &&
       ok;
  ok = expect(response.host_scan.ticket == "CSD_TICKET_SEED",
              "host-scan parser should expose ticket metadata") &&
       ok;
  ok = expect(response.host_scan.token == "CSD_TOKEN_SEED",
              "host-scan parser should expose token metadata") &&
       ok;
  ok = expect(response.host_scan.base_uri == "/+CSCOE+/sdesktop/",
              "host-scan parser should expose base URI") &&
       ok;
  ok = expect(response.host_scan.wait_uri == "/+CSCOE+/sdesktop/wait.html",
              "host-scan parser should expose wait URI") &&
       ok;
  return ok;
}

bool test_non_success_shapes_take_precedence_over_complete_marker() {
  using namespace ecnuvpn::vpn_engine::protocol;

  const std::string complete_error_xml =
      "<config-auth client=\"vpn\" type=\"complete\">"
      "<auth id=\"error\"><message>Invalid credentials</message></auth>"
      "<error>Login failed</error>"
      "</config-auth>";
  const std::string complete_host_scan_xml =
      "<config-auth client=\"vpn\" type=\"complete\">"
      "<auth id=\"main\"><message>Host scan required</message></auth>"
      "<host-scan ticket=\"CSD_TICKET_SEED\" token=\"CSD_TOKEN_SEED\" />"
      "</config-auth>";

  AggregateAuthResponse complete_error;
  const auto parsed_error =
      parse_aggregate_auth_response(complete_error_xml, &complete_error);
  AggregateAuthResponse complete_host_scan;
  const auto parsed_host_scan =
      parse_aggregate_auth_response(complete_host_scan_xml, &complete_host_scan);

  bool ok = true;
  ok = expect(parsed_error.ok, "complete error XML should parse") && ok;
  ok = expect(complete_error.type == AggregateAuthResponseType::error,
              "error shape should take precedence over complete marker") &&
       ok;
  ok = expect(parsed_host_scan.ok, "complete host-scan XML should parse") && ok;
  ok = expect(complete_host_scan.type == AggregateAuthResponseType::host_scan,
              "host-scan shape should take precedence over complete marker") &&
       ok;
  return ok;
}

bool test_rejects_html_and_oversized_responses() {
  using namespace ecnuvpn::vpn_engine::protocol;

  AggregateAuthResponse response;
  const auto html =
      parse_aggregate_auth_response("<html><form></form></html>", &response);
  const auto too_large =
      parse_aggregate_auth_response(std::string(1024 * 1024 + 1, 'x'),
                                    &response);

  bool ok = true;
  ok = expect(!html.ok, "HTML response should fail") && ok;
  ok = expect(html.code == "auth_protocol_mismatch",
              "HTML should map to auth_protocol_mismatch") &&
       ok;
  ok = expect(!too_large.ok, "oversized XML response should fail") && ok;
  ok = expect(too_large.code == "auth_response_too_large",
              "oversized XML should map to stable code") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = test_builds_deterministic_init_xml() && ok;
  ok = test_parses_auth_request_and_echoes_opaque() && ok;
  ok = test_parses_success_token_without_formatting_cookie() && ok;
  ok = test_parses_challenge_and_error_shapes() && ok;
  ok = test_parses_group_select_options() && ok;
  ok = test_select_option_without_value_uses_visible_text() && ok;
  ok = test_parses_host_scan_metadata() && ok;
  ok = test_non_success_shapes_take_precedence_over_complete_marker() && ok;
  ok = test_rejects_html_and_oversized_responses() && ok;
  return ok ? 0 : 1;
}
