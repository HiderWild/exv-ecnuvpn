#include "cli/cli_commands.hpp"

#include <nlohmann/json.hpp>

#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

#ifndef EXV_SOURCE_DIR
#define EXV_SOURCE_DIR "."
#endif

namespace {

using json = nlohmann::json;

bool expect(bool condition, const std::string &message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

struct SentRequest {
  std::string action;
  json payload;
};

struct Fixture {
  std::vector<SentRequest> sent;
  std::ostringstream out;
  std::ostringstream err;
  bool confirm_value = false;
  int confirm_calls = 0;
  int resolve_calls = 0;
  int version_probe_calls = 0;

  exv::cli::CliCommandDeps deps() {
    exv::cli::CliCommandDeps d;
    d.out = &out;
    d.err = &err;
    d.resolve_core = [this]() {
      ++resolve_calls;
      exv::cli::CliCoreResolution resolution;
      resolution.ok = true;
      resolution.ipc_path = "fake-pipe";
      return resolution;
    };
    d.send_core_request = [this](const std::string &, const std::string &request_line) {
      auto request = json::parse(request_line);
      sent.push_back(SentRequest{
          request.value("action", std::string()),
          json::parse(request.value("payload_json", std::string("{}")))});
      return json{{"request_id", request.value("request_id", std::string())},
                  {"success", true},
                  {"payload_json", json{{"ok", true}}.dump()}}
          .dump();
    };
    d.confirm = [this](const std::string &) {
      ++confirm_calls;
      return confirm_value;
    };
    d.version_probe = [this]() {
      ++version_probe_calls;
      return std::string("exv 3.3.0\n");
    };
    return d;
  }
};

bool expect_action(const SentRequest &request, const std::string &action) {
  return expect(request.action == action,
                "expected action " + action + ", got " + request.action);
}

std::string read_source_file(const std::filesystem::path &relative_path) {
  std::ifstream in(std::filesystem::path(EXV_SOURCE_DIR) / relative_path);
  std::ostringstream buffer;
  buffer << in.rdbuf();
  return buffer.str();
}

} // namespace

int main() {
  bool ok = true;

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv"}, deps);
    ok = expect(rc == 0, "exv without arguments should default to start") &&
         ok;
    ok = expect(fixture.sent.size() == 1,
                "exv without arguments should send one request") &&
         ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "vpn.connect") && ok;
    }
    ok = expect(fixture.out.str().find("VPN connect request submitted") !=
                    std::string::npos,
                "exv without arguments should print a visible connect acknowledgement") &&
         ok;
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "start"}, deps);
    ok = expect(rc == 0, "start should succeed") && ok;
    ok = expect(fixture.sent.size() == 1, "start should send one request") && ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "vpn.connect") && ok;
    }
    ok = expect(fixture.out.str().find("VPN connect request submitted") !=
                    std::string::npos,
                "start should print a visible connect acknowledgement") &&
         ok;
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "start", "-rt", "3"}, deps);
    ok = expect(rc == 0, "start -rt 3 should succeed") && ok;
    ok = expect(fixture.sent.size() == 2, "start -rt 3 should send two requests") && ok;
    if (fixture.sent.size() == 2) {
      ok = expect_action(fixture.sent[0], "config.saveSettings") && ok;
      ok = expect(fixture.sent[0].payload["settings"]["retry_limit"] == 3,
                  "retry_limit should be 3") && ok;
      ok = expect_action(fixture.sent[1], "vpn.connect") && ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "start", "-rt"}, deps);
    ok = expect(rc == 0, "start -rt should succeed") && ok;
    ok = expect(fixture.sent.size() == 2, "start -rt should send two requests") && ok;
    if (fixture.sent.size() == 2) {
      ok = expect(fixture.sent[0].payload["settings"]["retry_limit"] == -1,
                  "retry_limit should be -1") && ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "stop"}, deps);
    ok = expect(rc == 0, "stop should succeed") && ok;
    ok = expect(fixture.sent.size() == 1, "stop should send one request") && ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "vpn.disconnect") && ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "status"}, deps);
    ok = expect(rc == 0, "status should succeed") && ok;
    ok = expect(fixture.sent.size() == 1, "status should send one request") && ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "status.get") && ok;
    }
    ok = expect(fixture.out.str().find("\"ok\"") != std::string::npos,
                "status should print the core payload instead of returning silently") &&
         ok;
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "config", "show"}, deps);
    ok = expect(rc == 0, "config show should succeed") && ok;
    ok = expect(fixture.sent.size() == 1, "config show should send one request") && ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "config.get") && ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "config", "set", "server", "https://vpn.example.edu"}, deps);
    ok = expect(rc == 0, "config set server should succeed") && ok;
    ok = expect(fixture.sent.size() == 1, "config set server should send one request") && ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "config.saveAuth") && ok;
      ok = expect(fixture.sent[0].payload["server"] == "https://vpn.example.edu",
                  "server should be in auth payload") && ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "config", "set", "mtu", "1400"}, deps);
    ok = expect(rc == 0, "config set mtu should succeed") && ok;
    ok = expect(fixture.sent.size() == 1, "config set mtu should send one request") && ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "config.saveSettings") && ok;
      ok = expect(fixture.sent[0].payload["settings"]["mtu"] == 1400,
                  "mtu should be numeric in settings payload") && ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command(
        {"exv-cli", "config", "set", "include_class_a_private_routes", "true"},
        deps);
    ok = expect(rc == 0,
                "config set include_class_a_private_routes should succeed") &&
         ok;
    ok = expect(fixture.sent.size() == 1,
                "config set include_class_a_private_routes should send one request") &&
         ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "config.saveSettings") && ok;
      ok = expect(fixture.sent[0].payload["settings"]["include_class_a_private_routes"] == true,
                  "class A route toggle should be boolean in settings payload") &&
           ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command(
        {"exv-cli", "config", "routes", "add", "10.0.0.0/8"}, deps);
    ok = expect(rc == 0, "config routes add should succeed") && ok;
    ok = expect(fixture.sent.size() == 1,
                "config routes add should send one request") &&
         ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "routes.add") && ok;
      ok = expect(fixture.sent[0].payload["cidr"] == "10.0.0.0/8",
                  "routes.add should send cidr payload") &&
           ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc =
        exv::cli::run_cli_command({"exv-cli", "config", "key", "status"}, deps);
    ok = expect(rc == 0, "config key status should succeed") && ok;
    ok = expect(fixture.sent.size() == 1,
                "config key status should send one request") &&
         ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "key.status") && ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "logs"}, deps);
    ok = expect(rc == 0, "logs should succeed") && ok;
    ok = expect(fixture.sent.size() == 1, "logs should send one request") &&
         ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "logs.list") && ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "config", "set", "unknown", "value"}, deps);
    ok = expect(rc != 0, "unknown config key should fail locally") && ok;
    ok = expect(fixture.sent.empty(), "unknown config key should send no request") && ok;
  }

  {
    Fixture fixture;
    fixture.confirm_value = false;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "config", "reset"}, deps);
    ok = expect(rc != 0, "declined config reset should fail") && ok;
    ok = expect(fixture.confirm_calls == 1, "config reset should prompt once") && ok;
    ok = expect(fixture.sent.empty(), "declined config reset should send no request") && ok;
  }

  {
    Fixture fixture;
    fixture.confirm_value = true;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "config", "reset"}, deps);
    ok = expect(rc == 0, "confirmed config reset should succeed") && ok;
    ok = expect(fixture.confirm_calls == 1, "config reset should prompt once") && ok;
    ok = expect(fixture.sent.size() == 1, "confirmed config reset should send one request") && ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "config.reset") && ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    deps.interactive = false;
    int rc = exv::cli::run_cli_command({"exv-cli", "config", "reset"}, deps);
    ok = expect(rc != 0, "non-interactive config reset without confirm should fail") && ok;
    ok = expect(fixture.confirm_calls == 0, "non-interactive config reset should not prompt") && ok;
    ok = expect(fixture.sent.empty(), "non-interactive config reset should send no request") && ok;
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    deps.interactive = false;
    int rc = exv::cli::run_cli_command({"exv-cli", "config", "reset", "--confirm"}, deps);
    ok = expect(rc == 0, "non-interactive config reset with confirm should succeed") && ok;
    ok = expect(fixture.sent.size() == 1, "confirmed non-interactive reset should send one request") && ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "config.reset") && ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv", "help"}, deps);
    ok = expect(rc == 0, "exv help should succeed") && ok;
    ok = expect(fixture.out.str().find("Usage: exv ") != std::string::npos,
                "exv help should use the invoked program name") &&
         ok;
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "version"}, deps);
    ok = expect(rc == 0, "version should succeed") && ok;
    ok = expect(fixture.version_probe_calls == 1, "version should perform discovery probe") && ok;
    ok = expect(fixture.sent.empty(), "version should send no business action") && ok;
    ok = expect(fixture.out.str().find("exv 3.3.0") != std::string::npos,
                "version should print core version probe output") && ok;
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "service", "install"}, deps);
    ok = expect(rc == 0, "service install should succeed") && ok;
    ok = expect(fixture.sent.size() == 1, "service install should send one request") && ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "service.install") && ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "service", "uninstall"}, deps);
    ok = expect(rc == 0, "service uninstall should succeed") && ok;
    ok = expect(fixture.sent.size() == 1, "service uninstall should send one request") && ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "service.uninstall") && ok;
    }
  }

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "service", "status"}, deps);
    ok = expect(rc == 0, "service status should succeed") && ok;
    ok = expect(fixture.sent.size() == 1, "service status should send one request") && ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "service.status") && ok;
    }
  }

  {
    const std::string app_main = read_source_file("src/app/main.cpp");
    ok = expect(app_main.find("Use exv-cli for user commands") ==
                    std::string::npos,
                "exv app entrypoint should not print core-only CLI guidance") &&
         ok;
    ok = expect(app_main.find("run_cli_entrypoint") != std::string::npos,
                "exv app entrypoint should delegate user commands to cli parser") &&
         ok;
    ok = expect(app_main.find("--mode=core") != std::string::npos,
                "exv app entrypoint should keep explicit core mode") &&
         ok;
    ok = expect(app_main.find("desktop-rpc") != std::string::npos,
                "exv app entrypoint should keep desktop RPC mode") &&
         ok;
  }

  if (ok) {
    std::cout << "cli_commands_test: all assertions passed\n";
  } else {
    std::cerr << "cli_commands_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
