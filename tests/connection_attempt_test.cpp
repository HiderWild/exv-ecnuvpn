#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "app_api.hpp"
#include "core/config/config.hpp"
#include "connection_attempt.hpp"

#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <string>

namespace {

static const char *MOCK_PASSWORD = "test-mock-password-placeholder";

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

std::filesystem::path unique_temp_dir(const std::string &name) {
  const auto stamp =
      std::chrono::steady_clock::now().time_since_epoch().count();
  auto path = std::filesystem::temp_directory_path() /
              ("ecnuvpn-" + name + "-" + std::to_string(stamp));
  std::filesystem::create_directories(path);
  return path;
}

struct RuntimePathGuard {
  ~RuntimePathGuard() { ecnuvpn::platform::clear_runtime_path_override(); }
};

void write_json_file(const std::filesystem::path &path,
                     const nlohmann::json &json) {
  std::filesystem::create_directories(path.parent_path());
  std::ofstream out(path, std::ios::out | std::ios::trunc);
  out << json.dump(2);
}

void write_registry_record(const std::filesystem::path &dir, int owner_pid,
                           const std::string &attempt_id,
                           const std::string &state = "active",
                           int helper_pid = -1, int supervisor_pid = -1) {
  write_json_file(dir / "connect-attempt.json",
                  nlohmann::json{{"attempt_id", attempt_id},
                                 {"owner_pid", owner_pid},
                                 {"helper_pid", helper_pid},
                                 {"supervisor_pid", supervisor_pid},
                                 {"mode", "native_auth"},
                                 {"created_at_unix_ms", 1712345678000LL},
                                 {"state", state},
                                 {"terminal_reason", ""}});
}

void write_lock_owner(const std::filesystem::path &dir, int owner_pid,
                      const std::string &attempt_id) {
  const auto lock_dir = dir / "connect-attempt.lock";
  std::filesystem::create_directories(lock_dir);
  write_json_file(lock_dir / "owner.json",
                  nlohmann::json{{"attempt_id", attempt_id},
                                 {"owner_pid", owner_pid},
                                 {"helper_pid", -1},
                                 {"supervisor_pid", -1},
                                 {"mode", "native_auth"},
                                 {"created_at_unix_ms", 1712345679000LL},
                                 {"state", "active"},
                                 {"terminal_reason", ""}});
}

bool active_owner_blocks_second_attempt() {
  namespace attempt = ecnuvpn::connection_attempt;

  const auto dir = unique_temp_dir("active-attempt");
  attempt::AcquireOptions first;
  first.config_dir = dir.string();
  first.mode = "native_auth";
  first.owner_pid = 101;
  first.is_process_alive = [](int pid) { return pid == 101; };

  const auto acquired = attempt::try_acquire(first);

  attempt::AcquireOptions second = first;
  second.owner_pid = 202;
  second.is_process_alive = [](int pid) { return pid == 101; };
  const auto blocked = attempt::try_acquire(second);

  bool ok = true;
  ok = expect(acquired.acquired, "first attempt should acquire the guard") && ok;
  ok = expect(acquired.record.owner_pid == 101,
              "first attempt should persist owner pid") &&
       ok;
  ok = expect(acquired.record.mode == "native_auth",
              "first attempt should persist mode") &&
       ok;
  ok = expect(!blocked.acquired,
              "second attempt should not acquire while owner is alive") &&
       ok;
  ok = expect(blocked.code == attempt::kConnectionAttemptActiveCode,
              "blocked attempt should return typed active code") &&
       ok;
  ok = expect(blocked.record.attempt_id == acquired.record.attempt_id,
              "blocked attempt should expose current attempt record") &&
       ok;
  ok = expect(blocked.record.owner_pid == 101,
              "blocked attempt should not replace active owner") &&
       ok;
  return ok;
}

bool dead_owner_allows_stale_attempt_replacement() {
  namespace attempt = ecnuvpn::connection_attempt;

  const auto dir = unique_temp_dir("stale-attempt");
  attempt::AcquireOptions stale;
  stale.config_dir = dir.string();
  stale.owner_pid = 303;
  stale.is_process_alive = [](int pid) { return pid == 303; };
  const auto first = attempt::try_acquire(stale);

  attempt::AcquireOptions replacement = stale;
  replacement.owner_pid = 404;
  replacement.is_process_alive = [](int) { return false; };
  const auto second = attempt::try_acquire(replacement);

  bool ok = true;
  ok = expect(first.acquired, "stale setup should acquire initial attempt") && ok;
  ok = expect(second.acquired,
              "dead owner should allow stale attempt replacement") &&
       ok;
  ok = expect(second.record.owner_pid == 404,
              "replacement should persist the new owner pid") &&
       ok;
  ok = expect(second.record.attempt_id != first.record.attempt_id,
              "replacement should create a new attempt id") &&
       ok;
  return ok;
}

bool unknown_owner_liveness_keeps_attempt_active() {
  namespace attempt = ecnuvpn::connection_attempt;

  const auto dir = unique_temp_dir("unknown-liveness-attempt");
  attempt::AcquireOptions active;
  active.config_dir = dir.string();
  active.owner_pid = 707;
  active.is_process_alive = [](int pid) { return pid == 707; };
  const auto first = attempt::try_acquire(active);

  attempt::AcquireOptions contender = active;
  contender.owner_pid = 808;
  contender.probe_process_liveness = [](int) {
    return attempt::ProcessLiveness::unknown;
  };
  const auto second = attempt::try_acquire(contender);

  bool ok = true;
  ok = expect(first.acquired,
              "unknown liveness setup should acquire initial attempt") &&
       ok;
  ok = expect(!second.acquired,
              "unknown owner liveness must keep active attempt locked") &&
       ok;
  ok = expect(second.code == attempt::kConnectionAttemptActiveCode,
              "unknown liveness should report active attempt") &&
       ok;
  ok = expect(second.record.owner_pid == 707,
              "unknown liveness must not replace active owner") &&
       ok;
  return ok;
}

bool fresh_lock_owner_metadata_wins_over_stale_registry() {
  namespace attempt = ecnuvpn::connection_attempt;

  const auto dir = unique_temp_dir("fresh-lock-over-stale-record");
  write_registry_record(dir, 111, "old-dead-attempt");
  write_lock_owner(dir, 222, "fresh-live-attempt");

  attempt::AcquireOptions contender;
  contender.config_dir = dir.string();
  contender.owner_pid = 333;
  contender.probe_process_liveness = [](int pid) {
    if (pid == 111)
      return attempt::ProcessLiveness::dead;
    if (pid == 222)
      return attempt::ProcessLiveness::alive;
    return attempt::ProcessLiveness::unknown;
  };

  const auto result = attempt::try_acquire(contender);

  bool ok = true;
  ok = expect(!result.acquired,
              "fresh lock owner should block stale cleanup contender") &&
       ok;
  ok = expect(result.code == attempt::kConnectionAttemptActiveCode,
              "fresh lock owner should report active attempt") &&
       ok;
  ok = expect(result.record.attempt_id == "fresh-live-attempt",
              "fresh lock metadata should win over stale registry") &&
       ok;
  ok = expect(result.record.owner_pid == 222,
              "fresh lock metadata should preserve live owner pid") &&
       ok;
  return ok;
}

bool dead_owner_live_helper_blocks_stale_cleanup() {
  namespace attempt = ecnuvpn::connection_attempt;

  const auto dir = unique_temp_dir("dead-owner-live-helper");
  write_registry_record(dir, 111, "helper-owned-attempt", "active", 222, -1);
  write_lock_owner(dir, 111, "helper-owned-attempt");

  attempt::AcquireOptions contender;
  contender.config_dir = dir.string();
  contender.owner_pid = 333;
  contender.probe_process_liveness = [](int pid) {
    if (pid == 111)
      return attempt::ProcessLiveness::dead;
    if (pid == 222)
      return attempt::ProcessLiveness::alive;
    return attempt::ProcessLiveness::unknown;
  };

  const auto result = attempt::try_acquire(contender);

  bool ok = true;
  ok = expect(!result.acquired,
              "dead desktop owner must not clear a live helper attempt") &&
       ok;
  ok = expect(result.code == attempt::kConnectionAttemptActiveCode,
              "live helper should report active attempt") &&
       ok;
  ok = expect(result.record.attempt_id == "helper-owned-attempt",
              "live helper should preserve current attempt id") &&
       ok;
  ok = expect(result.record.helper_pid == 222,
              "active result should expose recorded helper pid") &&
       ok;
  return ok;
}

bool dead_owner_live_supervisor_blocks_stale_cleanup() {
  namespace attempt = ecnuvpn::connection_attempt;

  const auto dir = unique_temp_dir("dead-owner-live-supervisor");
  write_registry_record(dir, 121, "supervisor-owned-attempt", "active", -1,
                        232);
  write_lock_owner(dir, 121, "supervisor-owned-attempt");

  attempt::AcquireOptions contender;
  contender.config_dir = dir.string();
  contender.owner_pid = 343;
  contender.probe_process_liveness = [](int pid) {
    if (pid == 121)
      return attempt::ProcessLiveness::dead;
    if (pid == 232)
      return attempt::ProcessLiveness::alive;
    return attempt::ProcessLiveness::unknown;
  };

  const auto result = attempt::try_acquire(contender);

  bool ok = true;
  ok = expect(!result.acquired,
              "dead desktop owner must not clear a live supervisor attempt") &&
       ok;
  ok = expect(result.code == attempt::kConnectionAttemptActiveCode,
              "live supervisor should report active attempt") &&
       ok;
  ok = expect(result.record.attempt_id == "supervisor-owned-attempt",
              "live supervisor should preserve current attempt id") &&
       ok;
  ok = expect(result.record.supervisor_pid == 232,
              "active result should expose recorded supervisor pid") &&
       ok;
  return ok;
}

bool dead_owner_dead_children_allow_stale_cleanup() {
  namespace attempt = ecnuvpn::connection_attempt;

  const auto dir = unique_temp_dir("dead-owner-dead-children");
  write_registry_record(dir, 131, "fully-dead-attempt", "active", 242, 253);
  write_lock_owner(dir, 131, "fully-dead-attempt");

  attempt::AcquireOptions contender;
  contender.config_dir = dir.string();
  contender.owner_pid = 464;
  contender.probe_process_liveness = [](int pid) {
    if (pid == 131 || pid == 242 || pid == 253)
      return attempt::ProcessLiveness::dead;
    return attempt::ProcessLiveness::alive;
  };

  const auto result = attempt::try_acquire(contender);

  bool ok = true;
  ok = expect(result.acquired,
              "fully dead attempt should be stale-cleared for replacement") &&
       ok;
  ok = expect(result.record.owner_pid == 464,
              "replacement should persist new owner after stale cleanup") &&
       ok;
  ok = expect(result.record.attempt_id != "fully-dead-attempt",
              "replacement should allocate a new attempt id") &&
       ok;
  return ok;
}

bool unknown_child_liveness_keeps_attempt_active() {
  namespace attempt = ecnuvpn::connection_attempt;

  const auto dir = unique_temp_dir("unknown-child-liveness");
  write_registry_record(dir, 141, "unknown-child-attempt", "active", 252, -1);
  write_lock_owner(dir, 141, "unknown-child-attempt");

  attempt::AcquireOptions contender;
  contender.config_dir = dir.string();
  contender.owner_pid = 363;
  contender.probe_process_liveness = [](int pid) {
    if (pid == 141)
      return attempt::ProcessLiveness::dead;
    if (pid == 252)
      return attempt::ProcessLiveness::unknown;
    return attempt::ProcessLiveness::dead;
  };

  const auto result = attempt::try_acquire(contender);

  bool ok = true;
  ok = expect(!result.acquired,
              "unknown child liveness must keep existing attempt active") &&
       ok;
  ok = expect(result.code == attempt::kConnectionAttemptActiveCode,
              "unknown child liveness should report active attempt") &&
       ok;
  ok = expect(result.record.attempt_id == "unknown-child-attempt",
              "unknown child liveness should preserve current attempt id") &&
       ok;
  return ok;
}

bool record_write_failure_removes_owned_lock_for_retry() {
  namespace attempt = ecnuvpn::connection_attempt;

  const auto dir = unique_temp_dir("write-failure-cleanup");
  std::filesystem::create_directories(dir / "connect-attempt.json");
  std::ofstream blocker(dir / "connect-attempt.json" / "blocker.txt",
                        std::ios::out | std::ios::trunc);
  blocker << "not a regular registry file";
  blocker.close();

  attempt::AcquireOptions first_options;
  first_options.config_dir = dir.string();
  first_options.owner_pid = 444;
  const auto failed = attempt::try_acquire(first_options);

  std::filesystem::remove_all(dir / "connect-attempt.json");

  attempt::AcquireOptions second_options = first_options;
  second_options.owner_pid = 555;
  const auto retried = attempt::try_acquire(second_options);

  bool ok = true;
  ok = expect(!failed.acquired,
              "record write failure should not report acquired guard") &&
       ok;
  ok = expect(failed.code == attempt::kConnectionAttemptLockFailedCode,
              "record write failure should return lock failure code") &&
       ok;
  ok = expect(retried.acquired,
              "record write failure should remove owned lock for retry") &&
       ok;
  ok = expect(retried.record.owner_pid == 555,
              "retry after write failure should acquire new owner") &&
       ok;
  return ok;
}

bool missing_registry_with_dead_lock_owner_is_recoverable() {
  namespace attempt = ecnuvpn::connection_attempt;

  const auto dir = unique_temp_dir("missing-registry-dead-lock-owner");
  write_lock_owner(dir, 666, "dead-lock-owner");

  attempt::AcquireOptions options;
  options.config_dir = dir.string();
  options.owner_pid = 777;
  options.probe_process_liveness = [](int pid) {
    return pid == 666 ? attempt::ProcessLiveness::dead
                      : attempt::ProcessLiveness::alive;
  };

  const auto result = attempt::try_acquire(options);

  bool ok = true;
  ok = expect(result.acquired,
              "dead lock owner without registry should be recoverable") &&
       ok;
  ok = expect(result.record.owner_pid == 777,
              "recovered missing registry should persist new owner") &&
       ok;
  return ok;
}

bool terminal_attempt_releases_guard_and_preserves_reason() {
  namespace attempt = ecnuvpn::connection_attempt;

  const auto dir = unique_temp_dir("terminal-attempt");
  attempt::AcquireOptions options;
  options.config_dir = dir.string();
  options.owner_pid = 505;
  options.is_process_alive = [](int pid) { return pid == 505; };
  const auto first = attempt::try_acquire(options);

  const bool terminal = attempt::mark_terminal_if_current(
      dir.string(), first.record.attempt_id, "helper_start_failed");

  attempt::AttemptRecord terminal_record;
  const bool read_terminal =
      attempt::read_record(dir.string(), &terminal_record);

  attempt::AcquireOptions next = options;
  next.owner_pid = 606;
  const auto second = attempt::try_acquire(next);

  bool ok = true;
  ok = expect(first.acquired, "initial attempt should acquire guard") && ok;
  ok = expect(terminal, "mark terminal should succeed for current attempt") && ok;
  ok = expect(read_terminal, "terminal record should remain readable") && ok;
  ok = expect(terminal_record.state == "terminal",
              "terminal record should persist terminal state") &&
       ok;
  ok = expect(terminal_record.terminal_reason == "helper_start_failed",
              "terminal record should persist terminal reason") &&
       ok;
  ok = expect(second.acquired,
              "terminal attempt should release guard for next attempt") &&
       ok;
  return ok;
}

bool scope_exit_marks_current_attempt_terminal_on_exception() {
  namespace attempt = ecnuvpn::connection_attempt;

  const auto dir = unique_temp_dir("exception-cleanup-attempt");
  attempt::AcquireOptions options;
  options.config_dir = dir.string();
  options.owner_pid = 909;
  options.is_process_alive = [](int pid) { return pid == 909; };
  const auto first = attempt::try_acquire(options);

  bool caught = false;
  try {
    attempt::TerminalAttemptScope cleanup(dir.string(), first.record.attempt_id,
                                          "exception_after_acquire");
    throw std::runtime_error("forced exception after acquire");
    cleanup.dismiss();
  } catch (const std::runtime_error &) {
    caught = true;
  }

  attempt::AttemptRecord terminal_record;
  const bool read_terminal =
      attempt::read_record(dir.string(), &terminal_record);

  attempt::AcquireOptions next = options;
  next.owner_pid = 1001;
  next.is_process_alive = [](int pid) { return pid == 909 || pid == 1001; };
  const auto second = attempt::try_acquire(next);

  bool ok = true;
  ok = expect(first.acquired, "initial exception test attempt should acquire") &&
       ok;
  ok = expect(caught, "test should catch forced exception") && ok;
  ok = expect(read_terminal, "exception cleanup should leave terminal record") &&
       ok;
  ok = expect(terminal_record.state == "terminal",
              "exception cleanup should mark terminal state") &&
       ok;
  ok = expect(terminal_record.terminal_reason == "exception_after_acquire",
              "exception cleanup should preserve terminal reason") &&
       ok;
  ok = expect(second.acquired,
              "exception cleanup should release active guard for retry") &&
       ok;
  return ok;
}

bool app_api_native_connect_reports_active_attempt_before_bootstrap() {
  namespace attempt = ecnuvpn::connection_attempt;

  RuntimePathGuard guard;
  const auto dir = unique_temp_dir("app-api-active-attempt");

  ecnuvpn::Config cfg;
  cfg.server = "https://vpn.example.invalid";
  cfg.username = "student@example.invalid";
  cfg.password.clear();
  cfg.remember_password = false;
  cfg.vpn_engine = "native";
  cfg.openconnect_runtime = "bundled";
  cfg.windows_tunnel_driver = "tap";
  cfg.windows_tap_interface = "ECNU VPN TAP";
  cfg.auto_reconnect = false;
  cfg.extra_args.clear();

  {
    std::ofstream out(dir / "config.json", std::ios::out | std::ios::trunc);
    out << nlohmann::json(cfg).dump(2);
  }

  attempt::AcquireOptions active_options;
  active_options.config_dir = dir.string();
  active_options.mode = "native_auth";
  active_options.owner_pid = attempt::current_process_id();
  const auto active = attempt::try_acquire(active_options);

  const nlohmann::json payload{{"home", dir.string()},
                               {"config_dir", dir.string()},
                               {"password", MOCK_PASSWORD}};
  const auto response =
      ecnuvpn::app_api::handle_action("vpn.connect", payload);

  attempt::mark_terminal_if_current(dir.string(), active.record.attempt_id,
                                    "test_complete");

  bool ok = true;
  ok = expect(active.acquired,
              "app_api guard setup should acquire initial attempt") &&
       ok;
  ok = expect(response.is_object(), "app_api response should be an object") &&
       ok;
  ok = expect(!response.value("ok", true),
              "app_api should reject active native connect attempt") &&
       ok;
  ok = expect(response.value("code", std::string()) ==
                  attempt::kConnectionAttemptActiveCode,
              "app_api should preserve connection_attempt_active code") &&
       ok;
  ok = expect(response.contains("current_attempt") &&
                  response["current_attempt"].is_object(),
              "app_api active response should include current attempt") &&
       ok;
  if (response.contains("current_attempt") &&
      response["current_attempt"].is_object()) {
    ok = expect(response["current_attempt"].value("attempt_id",
                                                  std::string()) ==
                    active.record.attempt_id,
                "app_api active response should expose current attempt id") &&
         ok;
    ok = expect(response["current_attempt"].value("owner_pid", -1) ==
                    active.record.owner_pid,
                "app_api active response should expose current owner pid") &&
         ok;
  }
  return ok;
}

bool app_api_constructs_attempt_cleanup_before_timing_mark() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const auto app_api_path =
      std::filesystem::path(ECNUVPN_SOURCE_DIR) / "src" / "app_api.cpp";
  std::ifstream in(app_api_path);
  const std::string source((std::istreambuf_iterator<char>(in)),
                           std::istreambuf_iterator<char>());

  const auto acquire =
      source.find("connection_attempt::AcquireResult attempt =");
  const auto cleanup =
      source.find("connection_attempt::TerminalAttemptScope attempt_cleanup");
  const auto timing = source.find("timing.mark(\"connection_attempt\"");

  bool ok = true;
  ok = expect(acquire != std::string::npos,
              "app_api should acquire native attempt guard") &&
       ok;
  ok = expect(cleanup != std::string::npos,
              "app_api should construct terminal cleanup scope") &&
       ok;
  ok = expect(timing != std::string::npos,
              "app_api should keep connection attempt timing mark") &&
       ok;
  if (acquire != std::string::npos && cleanup != std::string::npos &&
      timing != std::string::npos) {
    ok = expect(acquire < cleanup,
                "cleanup scope should be after successful acquire") &&
         ok;
    ok = expect(cleanup < timing,
                "cleanup scope should be constructed before timing mark") &&
         ok;
  }
  return ok;
#endif
}

} // namespace

namespace ecnuvpn {
namespace helper {

bool is_available() { return false; }

} // namespace helper
} // namespace ecnuvpn

int main() {
  bool ok = true;
  ok = active_owner_blocks_second_attempt() && ok;
  ok = dead_owner_allows_stale_attempt_replacement() && ok;
  ok = unknown_owner_liveness_keeps_attempt_active() && ok;
  ok = fresh_lock_owner_metadata_wins_over_stale_registry() && ok;
  ok = dead_owner_live_helper_blocks_stale_cleanup() && ok;
  ok = dead_owner_live_supervisor_blocks_stale_cleanup() && ok;
  ok = dead_owner_dead_children_allow_stale_cleanup() && ok;
  ok = unknown_child_liveness_keeps_attempt_active() && ok;
  ok = record_write_failure_removes_owned_lock_for_retry() && ok;
  ok = missing_registry_with_dead_lock_owner_is_recoverable() && ok;
  ok = terminal_attempt_releases_guard_and_preserves_reason() && ok;
  ok = scope_exit_marks_current_attempt_terminal_on_exception() && ok;
  ok = app_api_native_connect_reports_active_attempt_before_bootstrap() && ok;
  ok = app_api_constructs_attempt_cleanup_before_timing_mark() && ok;
  return ok ? 0 : 1;
}
