#include "cli/cli_commands.hpp"

#include <nlohmann/json.hpp>

#include <iostream>
#include <sstream>
#include <string>
#include <vector>

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

} // namespace

int main() {
  bool ok = true;

  {
    Fixture fixture;
    auto deps = fixture.deps();
    int rc = exv::cli::run_cli_command({"exv-cli", "start"}, deps);
    ok = expect(rc == 0, "start should succeed") && ok;
    ok = expect(fixture.sent.size() == 1, "start should send one request") && ok;
    if (fixture.sent.size() == 1) {
      ok = expect_action(fixture.sent[0], "vpn.connect") && ok;
    }
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
    int rc = exv::cli::run_cli_command({"exv-cli", "version"}, deps);
    ok = expect(rc == 0, "version should succeed") && ok;
    ok = expect(fixture.version_probe_calls == 1, "version should perform discovery probe") && ok;
    ok = expect(fixture.sent.empty(), "version should send no business action") && ok;
    ok = expect(fixture.out.str().find("exv 3.3.0") != std::string::npos,
                "version should print core version probe output") && ok;
  }

  if (ok) {
    std::cout << "cli_commands_test: all assertions passed\n";
  } else {
    std::cerr << "cli_commands_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
