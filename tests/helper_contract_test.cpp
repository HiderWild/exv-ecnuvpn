// Helper single-protocol contract tests.

#include "contracts/generated/system_contract.hpp"
#include "helper/common/helper_messages.hpp"
#include "helper/common/helper_protocol.hpp"
#include "helper/helper_handler.hpp"
#include "helper/helper_network_ops.hpp"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <chrono>
#include <condition_variable>
#include <functional>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <string_view>
#include <vector>
#include <memory>
#include <mutex>
#include <thread>

using json = nlohmann::json;

namespace ecnuvpn::logger {
void info(const std::string &) {}
} // namespace ecnuvpn::logger

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

bool wait_until(const std::function<bool()> &predicate,
                std::chrono::milliseconds timeout) {
  const auto deadline = std::chrono::steady_clock::now() + timeout;
  while (std::chrono::steady_clock::now() < deadline) {
    if (predicate()) {
      return true;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(5));
  }
  return predicate();
}

json read_json_file(const std::filesystem::path &path) {
  std::ifstream in(path);
  if (!in)
    throw std::runtime_error("failed to open " + path.string());
  json parsed;
  in >> parsed;
  return parsed;
}

std::string read_text_file(const std::filesystem::path &path) {
  std::ifstream in(path);
  if (!in)
    throw std::runtime_error("failed to open " + path.string());
  return std::string(std::istreambuf_iterator<char>(in),
                     std::istreambuf_iterator<char>());
}

template <typename Range>
bool contains(const Range &values, std::string_view value) {
  return std::find(values.begin(), values.end(), value) != values.end();
}

std::vector<std::string> helper_op_names(const json &helper) {
  std::vector<std::string> names;
  for (const auto &op : helper.at("ops")) {
    names.push_back(op.at("name").get<std::string>());
  }
  return names;
}

bool json_has_key_recursive(const json &node, std::string_view key) {
  if (node.is_object()) {
    for (auto it = node.begin(); it != node.end(); ++it) {
      if (it.key() == key)
        return true;
      if (json_has_key_recursive(it.value(), key))
        return true;
    }
  } else if (node.is_array()) {
    for (const auto &item : node) {
      if (json_has_key_recursive(item, key))
        return true;
    }
  }
  return false;
}

bool json_contains_credential_field(const json &node) {
  static constexpr std::string_view forbidden[] = {
      "password",      "passwd",       "cookie",      "token",
      "secret",        "credential",   "auth_key",    "auth_token",
      "authToken",     "session_cookie", "webvpn_cookie", "csrf_token",
      "bearer_token",  "api_key",      "apikey"};
  for (const auto field : forbidden) {
    if (json_has_key_recursive(node, field))
      return true;
  }
  return false;
}

exv::helper::HelperResponse dispatch_json(exv::helper::HelperHandler &handler,
                                          exv::helper::HelperOp op,
                                          const json &payload) {
  exv::helper::HelperRequest req;
  req.op = op;
  req.payload_json = payload.dump();
  return handler.handle(req);
}

exv::helper::HelperResponse
dispatch_json(exv::helper::HelperHandler &handler, exv::helper::HelperOp op,
              const json &payload,
              const exv::helper::HelperRequestContext &context) {
  exv::helper::HelperRequest req;
  req.op = op;
  req.payload_json = payload.dump();
  return handler.handle(req, context);
}

exv::helper::HelperRequestContext peer_context(const std::string &owner,
                                               int pid,
                                               unsigned int uid) {
  exv::helper::HelperRequestContext context;
  context.peer.verified = true;
  context.peer.owner = owner;
  context.peer.pid = pid;
  context.peer.uid = uid;
  context.peer.gid = uid;
  return context;
}

exv::helper::HelperRequestContext unverified_context() {
  return {};
}

exv::helper::AcquireCoreLeaseResponse
acquire_core_lease(exv::helper::HelperHandler &handler, int core_pid = 4321,
                   const std::string &purpose = "connect") {
  exv::helper::AcquireCoreLeaseRequest acquire_req;
  acquire_req.core_pid = core_pid;
  acquire_req.purpose = purpose;
  auto acquired = dispatch_json(handler, exv::helper::HelperOp::AcquireCoreLease,
                                json(acquire_req));
  if (!acquired.success) {
    return {};
  }
  return exv::helper::acquire_core_lease_response_from_json(
      json::parse(acquired.payload_json));
}

exv::helper::AcquireCoreLeaseResponse
acquire_core_lease(exv::helper::HelperHandler &handler,
                   const exv::helper::HelperRequestContext &context,
                   int core_pid = 4321,
                   const std::string &purpose = "connect") {
  exv::helper::AcquireCoreLeaseRequest acquire_req;
  acquire_req.core_pid = core_pid;
  acquire_req.purpose = purpose;
  auto acquired = dispatch_json(handler, exv::helper::HelperOp::AcquireCoreLease,
                                json(acquire_req), context);
  if (!acquired.success) {
    return {};
  }
  return exv::helper::acquire_core_lease_response_from_json(
      json::parse(acquired.payload_json));
}

exv::helper::StartSessionResponse
start_session_with_core_lease(exv::helper::HelperHandler &handler,
                              const std::string &profile_id = "profile-a") {
  auto acquired = acquire_core_lease(handler);
  if (!acquired.accepted) {
    return {};
  }

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = profile_id;
  auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                             json(start_req));
  if (!start.success) {
    return {};
  }
  return exv::helper::start_session_response_from_json(
      json::parse(start.payload_json));
}

class RecordingHelperNetworkOps final : public exv::helper::HelperNetworkOps {
public:
  exv::helper::PrepareTunnelDeviceResponse
  prepare_tunnel_device(
      const exv::helper::PrepareTunnelDeviceRequest &request,
      std::vector<exv::helper::ManagedResource> *created_resources) override {
    prepare_count++;
    last_prepare_session = request.session_id.value;
    exv::helper::PrepareTunnelDeviceResponse response;
    response.device_path = "helper-device://" + request.adapter_name;
    response.mtu = 1280;
    created_resources->push_back({"adapter", request.adapter_name});
    return response;
  }

  exv::helper::ApplyTunnelConfigResponse
  apply_tunnel_config(
      const exv::helper::ApplyTunnelConfigRequest &request,
      std::vector<exv::helper::ManagedResource> *created_resources) override {
    apply_count++;
    last_apply_session = request.config.session_id.value;
    for (const auto &route : request.config.routes) {
      created_resources->push_back({"route", route.destination});
    }
    for (const auto &server : request.config.dns.servers) {
      created_resources->push_back({"dns", server});
    }
    exv::helper::ApplyTunnelConfigResponse response;
    response.success = true;
    return response;
  }

  exv::helper::CleanupResponse cleanup(
      const exv::helper::SessionId &session_id,
      const exv::helper::CleanupPolicy &policy,
      const std::vector<exv::helper::ManagedResource> &resources) override {
    (void)policy;
    cleanup_count++;
    last_cleanup_session = session_id.value;
    last_cleanup_resource_count = resources.size();
    exv::helper::CleanupResponse response;
    response.success = cleanup_success;
    if (!response.success) {
      response.errors.push_back("recording cleanup failure");
    }
    return response;
  }

  bool cleanup_success = true;
  int prepare_count = 0;
  int apply_count = 0;
  int cleanup_count = 0;
  std::size_t last_cleanup_resource_count = 0;
  std::string last_prepare_session;
  std::string last_apply_session;
  std::string last_cleanup_session;
};

class RecordingHelperServiceOps final : public exv::helper::HelperServiceOps {
public:
  exv::helper::InstallServiceResponse
  install_service(const exv::helper::InstallServiceRequest &request) override {
    (void)request;
    ++install_count;
    exv::helper::InstallServiceResponse response;
    response.success = install_success;
    response.exit_code = install_success ? 0 : 7;
    response.message = install_success ? "installed" : "install failed";
    return response;
  }

  exv::helper::UninstallServiceResponse uninstall_service(
      const exv::helper::UninstallServiceRequest &request) override {
    (void)request;
    ++uninstall_count;
    exv::helper::UninstallServiceResponse response;
    response.success = uninstall_success;
    response.exit_code = uninstall_success ? 0 : 9;
    response.message =
        uninstall_success ? "uninstalled" : "uninstall failed";
    return response;
  }

  bool install_success = true;
  bool uninstall_success = true;
  int install_count = 0;
  int uninstall_count = 0;
};

class BlockingHelperNetworkOps final : public exv::helper::HelperNetworkOps {
public:
  exv::helper::PrepareTunnelDeviceResponse
  prepare_tunnel_device(
      const exv::helper::PrepareTunnelDeviceRequest &request,
      std::vector<exv::helper::ManagedResource> *created_resources) override {
    enter("prepare");
    {
      std::unique_lock<std::mutex> lock(mutex_);
      cv_.wait(lock, [&] { return release_prepare_; });
    }
    leave();

    exv::helper::PrepareTunnelDeviceResponse response;
    response.device_path = "helper-device://" + request.adapter_name;
    response.mtu = 1280;
    created_resources->push_back({"adapter", request.adapter_name});
    return response;
  }

  exv::helper::ApplyTunnelConfigResponse
  apply_tunnel_config(
      const exv::helper::ApplyTunnelConfigRequest &request,
      std::vector<exv::helper::ManagedResource> *created_resources) override {
    enter("apply");
    leave();

    for (const auto &route : request.config.routes) {
      created_resources->push_back({"route", route.destination});
    }

    exv::helper::ApplyTunnelConfigResponse response;
    response.success = true;
    return response;
  }

  exv::helper::CleanupResponse cleanup(
      const exv::helper::SessionId &session_id,
      const exv::helper::CleanupPolicy &policy,
      const std::vector<exv::helper::ManagedResource> &resources) override {
    (void)session_id;
    (void)policy;
    (void)resources;
    enter("cleanup");
    {
      std::unique_lock<std::mutex> lock(mutex_);
      cv_.wait(lock, [&] { return release_cleanup_; });
    }
    leave();

    exv::helper::CleanupResponse response;
    response.success = true;
    return response;
  }

  bool wait_for_prepare_started(std::chrono::milliseconds timeout) {
    std::unique_lock<std::mutex> lock(mutex_);
    return cv_.wait_for(lock, timeout, [&] { return prepare_started_; });
  }

  bool wait_for_cleanup_started(std::chrono::milliseconds timeout) {
    std::unique_lock<std::mutex> lock(mutex_);
    return cv_.wait_for(lock, timeout, [&] { return cleanup_started_; });
  }

  void release_prepare() {
    {
      std::lock_guard<std::mutex> lock(mutex_);
      release_prepare_ = true;
    }
    cv_.notify_all();
  }

  void release_cleanup() {
    {
      std::lock_guard<std::mutex> lock(mutex_);
      release_cleanup_ = true;
    }
    cv_.notify_all();
  }

  int max_active() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return max_active_;
  }

  std::vector<std::string> calls() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return calls_;
  }

private:
  void enter(const std::string &name) {
    std::lock_guard<std::mutex> lock(mutex_);
    ++active_;
    max_active_ = std::max(max_active_, active_);
    calls_.push_back(name);
    if (name == "prepare") {
      prepare_started_ = true;
    } else if (name == "cleanup") {
      cleanup_started_ = true;
    }
    cv_.notify_all();
  }

  void leave() {
    std::lock_guard<std::mutex> lock(mutex_);
    --active_;
    cv_.notify_all();
  }

  mutable std::mutex mutex_;
  std::condition_variable cv_;
  int active_ = 0;
  int max_active_ = 0;
  bool prepare_started_ = false;
  bool cleanup_started_ = false;
  bool release_prepare_ = false;
  bool release_cleanup_ = false;
  std::vector<std::string> calls_;
};

int test_manifest_declares_single_helper_protocol() {
  bool ok = true;
  const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  const auto manifest =
      read_json_file(source_dir / "contracts" / "system.contract.json");
  const auto &helper = manifest.at("modules").at("helper");

  ok = expect(!helper.contains("protocol_version"),
              "helper manifest must not expose protocol_version") &&
       ok;
  ok = expect(!helper.contains("legacy_ops"),
              "helper manifest must not declare legacy_ops") &&
       ok;

  const std::vector<std::string> expected_ops = {
      "Hello",        "StartSession",       "PrepareTunnelDevice",
      "ApplyTunnelConfig", "Heartbeat",     "Cleanup",
      "GetSnapshot",  "Shutdown",           "Inspect",
      "AcquireCoreLease", "KeepAlive",      "ReleaseCoreLease",
      "InstallService", "UninstallService", "ExportCleanupLease",
      "HandoffSession", "FinalizeHandoff"};
  ok = expect(helper_op_names(helper) == expected_ops,
              "helper ops must be the single protocol op set") &&
       ok;

  ok = expect(contains(helper.at("messages"), "ShutdownRequest"),
              "manifest must list ShutdownRequest") &&
       ok;
  ok = expect(contains(helper.at("messages"), "ShutdownResponse"),
              "manifest must list ShutdownResponse") &&
       ok;
  ok = expect(contains(helper.at("messages"), "InspectRequest"),
              "manifest must list InspectRequest") &&
       ok;
  ok = expect(contains(helper.at("messages"), "InspectResponse"),
              "manifest must list InspectResponse") &&
       ok;
  ok = expect(contains(helper.at("messages"), "AcquireCoreLeaseRequest"),
              "manifest must list AcquireCoreLeaseRequest") &&
       ok;
  ok = expect(contains(helper.at("messages"), "AcquireCoreLeaseResponse"),
              "manifest must list AcquireCoreLeaseResponse") &&
       ok;
  ok = expect(contains(helper.at("messages"), "KeepAliveRequest"),
              "manifest must list KeepAliveRequest") &&
       ok;
  ok = expect(contains(helper.at("messages"), "KeepAliveResponse"),
              "manifest must list KeepAliveResponse") &&
       ok;
  ok = expect(contains(helper.at("messages"), "ReleaseCoreLeaseRequest"),
              "manifest must list ReleaseCoreLeaseRequest") &&
       ok;
  ok = expect(contains(helper.at("messages"), "ReleaseCoreLeaseResponse"),
              "manifest must list ReleaseCoreLeaseResponse") &&
       ok;
  ok = expect(contains(helper.at("messages"), "InstallServiceRequest"),
              "manifest must list InstallServiceRequest") &&
       ok;
  ok = expect(contains(helper.at("messages"), "InstallServiceResponse"),
              "manifest must list InstallServiceResponse") &&
       ok;
  ok = expect(contains(helper.at("messages"), "UninstallServiceRequest"),
              "manifest must list UninstallServiceRequest") &&
       ok;
  ok = expect(contains(helper.at("messages"), "UninstallServiceResponse"),
              "manifest must list UninstallServiceResponse") &&
       ok;
  ok = expect(contains(helper.at("messages"), "ExportCleanupLeaseRequest"),
              "manifest must list ExportCleanupLeaseRequest") &&
       ok;
  ok = expect(contains(helper.at("messages"), "ExportCleanupLeaseResponse"),
              "manifest must list ExportCleanupLeaseResponse") &&
       ok;
  ok = expect(contains(helper.at("messages"), "HandoffSessionRequest"),
              "manifest must list HandoffSessionRequest") &&
       ok;
  ok = expect(contains(helper.at("messages"), "HandoffSessionResponse"),
              "manifest must list HandoffSessionResponse") &&
       ok;
  ok = expect(contains(helper.at("messages"), "FinalizeHandoffRequest"),
              "manifest must list FinalizeHandoffRequest") &&
       ok;
  ok = expect(contains(helper.at("messages"), "FinalizeHandoffResponse"),
              "manifest must list FinalizeHandoffResponse") &&
       ok;
  ok = expect(!contains(helper.at("messages"), "EndSessionRequest"),
              "manifest must not list EndSessionRequest") &&
       ok;
  ok = expect(!contains(helper.at("messages"), "EndSessionResponse"),
              "manifest must not list EndSessionResponse") &&
       ok;

  return ok ? 0 : 1;
}

int test_generated_contract_matches_helper_manifest() {
  bool ok = true;
  using namespace exv::contracts::generated;

  ok = expect(is_helper_op("Hello"), "generated helper ops include Hello") && ok;
  ok = expect(is_helper_op("Shutdown"),
              "generated helper ops include Shutdown") &&
       ok;
  ok = expect(is_helper_op("Inspect"),
              "generated helper ops include Inspect") &&
       ok;
  ok = expect(is_helper_op("AcquireCoreLease"),
              "generated helper ops include AcquireCoreLease") &&
       ok;
  ok = expect(is_helper_op("KeepAlive"),
              "generated helper ops include KeepAlive") &&
       ok;
  ok = expect(is_helper_op("ReleaseCoreLease"),
              "generated helper ops include ReleaseCoreLease") &&
       ok;
  ok = expect(is_helper_op("InstallService"),
              "generated helper ops include InstallService") &&
       ok;
  ok = expect(is_helper_op("UninstallService"),
              "generated helper ops include UninstallService") &&
       ok;
  ok = expect(is_helper_op("ExportCleanupLease"),
              "generated helper ops include ExportCleanupLease") &&
       ok;
  ok = expect(is_helper_op("HandoffSession"),
              "generated helper ops include HandoffSession") &&
       ok;
  ok = expect(is_helper_op("FinalizeHandoff"),
              "generated helper ops include FinalizeHandoff") &&
       ok;
  ok = expect(!is_helper_op("EndSession"),
              "generated helper ops exclude EndSession") &&
       ok;

  auto check = [&](std::string_view name, exv::helper::HelperOp op,
                   bool requires_session) {
    for (const auto &contract : HELPER_OP_CONTRACTS) {
      if (contract.name == name) {
        ok = expect(contract.code == static_cast<std::uint32_t>(op),
                    "generated op code must match wire enum") &&
             ok;
        ok = expect(contract.requires_session == requires_session,
                    "generated requires_session must match manifest") &&
             ok;
        return;
      }
    }
    std::cerr << "EXPECT FAILED: missing generated helper op " << name << '\n';
    ok = false;
  };

  check("Hello", exv::helper::HelperOp::Hello, false);
  check("StartSession", exv::helper::HelperOp::StartSession, false);
  check("PrepareTunnelDevice", exv::helper::HelperOp::PrepareTunnelDevice, true);
  check("ApplyTunnelConfig", exv::helper::HelperOp::ApplyTunnelConfig, true);
  check("Heartbeat", exv::helper::HelperOp::Heartbeat, true);
  check("Cleanup", exv::helper::HelperOp::Cleanup, true);
  check("GetSnapshot", exv::helper::HelperOp::GetSnapshot, false);
  check("Shutdown", exv::helper::HelperOp::Shutdown, true);
  check("Inspect", exv::helper::HelperOp::Inspect, false);
  check("AcquireCoreLease", exv::helper::HelperOp::AcquireCoreLease, false);
  check("KeepAlive", exv::helper::HelperOp::KeepAlive, false);
  check("ReleaseCoreLease", exv::helper::HelperOp::ReleaseCoreLease, false);
  check("InstallService", exv::helper::HelperOp::InstallService, false);
  check("UninstallService", exv::helper::HelperOp::UninstallService, false);
  check("ExportCleanupLease", exv::helper::HelperOp::ExportCleanupLease, false);
  check("HandoffSession", exv::helper::HelperOp::HandoffSession, false);
  check("FinalizeHandoff", exv::helper::HelperOp::FinalizeHandoff, false);

  return ok ? 0 : 1;
}

int test_daemon_uses_network_ops_handler_factory() {
  bool ok = true;
  const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  const auto helper_source = read_text_file(source_dir / "src" / "helper" /
                                           "helper.cpp");

  ok = expect(helper_source.find("create_helper_handler_for_daemon(options)") !=
                  std::string::npos,
              "helper daemon must construct handler through daemon factory") &&
       ok;
  ok = expect(helper_source.find(
                  "std::make_unique<exv::helper::HelperHandler>()") ==
                  std::string::npos,
              "helper daemon must not default-construct an unavailable handler") &&
       ok;

  return ok ? 0 : 1;
}

int test_daemon_does_not_hold_external_handler_lock() {
  bool ok = true;
  const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  const auto helper_source = read_text_file(source_dir / "src" / "helper" /
                                           "helper.cpp");

  ok = expect(helper_source.find("handler_mutex") == std::string::npos,
              "helper daemon must not hold an external handler lock across "
              "privileged task execution") &&
       ok;

  return ok ? 0 : 1;
}

int test_legacy_helper_internal_header_removed() {
  bool ok = true;
  const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  const auto header_path = source_dir / "src" / "helper" /
                           "helper_internal.hpp";
  ok = expect(!std::filesystem::exists(header_path),
              "unused legacy helper_internal.hpp must be removed") &&
       ok;
  return ok ? 0 : 1;
}

int test_unused_helper_server_stub_removed() {
  bool ok = true;
  const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  ok = expect(!std::filesystem::exists(source_dir / "src" / "helper" /
                                       "runtime" / "helper_server.hpp"),
              "unused helper_server.hpp production stub must be removed") &&
       ok;
  ok = expect(!std::filesystem::exists(source_dir / "src" / "helper" /
                                       "runtime" / "helper_server.cpp"),
              "unused helper_server.cpp production stub must be removed") &&
       ok;
  return ok ? 0 : 1;
}

bool is_source_file(const std::filesystem::path &path) {
  const auto ext = path.extension().string();
  return ext == ".cpp" || ext == ".hpp" || ext == ".h" || ext == ".inc" ||
         ext == ".cppm" || ext == ".ixx";
}

int test_helper_platform_boundary_lives_under_platform() {
  bool ok = true;
  const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);

  ok = expect(!std::filesystem::exists(source_dir / "src" / "helper" /
                                       "platform"),
              "helper platform implementation must live under src/platform") &&
       ok;

  const auto cmake = read_text_file(source_dir / "CMakeLists.txt");
  ok = expect(cmake.find("src/helper/platform") == std::string::npos,
              "CMake must not compile src/helper/platform sources") &&
       ok;

  const std::vector<std::filesystem::path> roots = {
      source_dir / "src",
      source_dir / "tests",
  };
  for (const auto &root : roots) {
    for (const auto &entry :
         std::filesystem::recursive_directory_iterator(root)) {
      if (!entry.is_regular_file() || !is_source_file(entry.path())) {
        continue;
      }
      const auto text = read_text_file(entry.path());
      ok = expect(text.find("#include \"helper/platform/") ==
                      std::string::npos,
                  "source files must include helper platform APIs from "
                  "platform/common") &&
           ok;
    }
  }

  const auto platform_root = source_dir / "src" / "platform";
  for (const auto &entry :
       std::filesystem::recursive_directory_iterator(platform_root)) {
    if (!entry.is_regular_file() || !is_source_file(entry.path())) {
      continue;
    }
    const auto text = read_text_file(entry.path());
    ok = expect(text.find("#include \"helper/runtime/") == std::string::npos,
                "platform must not include helper runtime internals") &&
         ok;
    ok = expect(text.find("#include \"helper/helper_handler") ==
                    std::string::npos,
                "platform must not include helper handler internals") &&
         ok;
  }

  return ok ? 0 : 1;
}

int test_oneshot_owner_is_uid_or_sid_not_pid_alias() {
  bool ok = true;
  const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  const std::vector<std::filesystem::path> files = {
      source_dir / "src" / "helper" / "helper.cpp",
      source_dir / "src" / "platform" / "win32" / "oneshot_bootstrap.cpp",
      source_dir / "src" / "platform" / "darwin" / "oneshot_bootstrap.cpp",
      source_dir / "src" / "platform" / "linux" / "oneshot_bootstrap.cpp"};

  for (const auto &file : files) {
    const auto text = read_text_file(file);
    ok = expect(text.find("pid:") == std::string::npos,
                "oneshot owner must not use pid: alias") &&
         ok;
  }
  return ok ? 0 : 1;
}

int test_windows_helper_has_no_second_service_entrypoint() {
  bool ok = true;
  const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  ok = expect(!std::filesystem::exists(source_dir / "src" / "helper" /
                                       "platform" / "win32" /
                                       "helper_service.cpp"),
              "Windows helper must not retain a second service entrypoint") &&
       ok;
  return ok ? 0 : 1;
}

int test_oneshot_entrypoint_uses_only_endpoint_argument() {
  bool ok = true;
  const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  const std::vector<std::filesystem::path> files = {
      source_dir / "src" / "helper" / "helper_main.cpp",
      source_dir / "src" / "platform" / "win32" / "oneshot_bootstrap.cpp",
      source_dir / "src" / "platform" / "darwin" / "oneshot_bootstrap.cpp",
      source_dir / "src" / "platform" / "linux" / "oneshot_bootstrap.cpp"};

  for (const auto &file : files) {
    const auto text = read_text_file(file);
    ok = expect(text.find("--endpoint") != std::string::npos,
                "oneshot startup code must use --endpoint") &&
         ok;
    ok = expect(text.find("--pipe") == std::string::npos,
                "oneshot startup code must not accept --pipe alias") &&
         ok;
    ok = expect(text.find("--socket") == std::string::npos,
                "oneshot startup code must not accept --socket alias") &&
         ok;
  }

  return ok ? 0 : 1;
}

int test_helper_connector_requires_explicit_endpoint_field() {
  bool ok = true;
  const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  const auto connector_source = read_text_file(
      source_dir / "src" / "helper" / "common" / "helper_connector.cpp");

  ok = expect(connector_source.find("config.helper_executable_path") ==
                  std::string::npos,
              "connector must not treat helper_executable_path as endpoint") &&
       ok;
  ok = expect(connector_source.find("legacy callers") == std::string::npos,
              "connector must not retain legacy endpoint resolution aliases") &&
       ok;

  return ok ? 0 : 1;
}

int test_hello_has_no_version_fields() {
  bool ok = true;

  exv::helper::HelloRequest req;
  json request_json = req;
  ok = expect(!json_has_key_recursive(request_json, "protocol_version"),
              "HelloRequest must not contain protocol_version") &&
       ok;
  ok = expect(!json_has_key_recursive(request_json, "client_version"),
              "HelloRequest must not contain client_version") &&
       ok;

  exv::helper::HelloResponse resp;
  resp.capabilities = {"tunnel_device_create", "route_apply", "dns_apply"};
  resp.mode = exv::helper::HelperMode::Transient;
  resp.startup_context.launch_mode = "oneshot";
  resp.startup_context.parent_pid = 1234;
  resp.session_state.active = false;
  resp.core_lease.active = true;
  resp.core_lease.lease_id = "lease-1";
  resp.core_lease.core_pid = 4321;
  resp.core_lease.purpose = "connect";
  resp.core_lease.last_seen_state = "authenticating";
  resp.task_queue.idle = false;
  resp.task_queue.current_job_id = "job-1";
  resp.task_queue.pending_jobs = 2;

  json response_json = resp;
  ok = expect(response_json.contains("capabilities"),
              "HelloResponse must include capabilities") &&
       ok;
  ok = expect(response_json.contains("mode"), "HelloResponse must include mode") &&
       ok;
  ok = expect(response_json.contains("startup_context"),
              "HelloResponse must include startup_context") &&
       ok;
  ok = expect(response_json.contains("session_state"),
              "HelloResponse must include session_state") &&
       ok;
  ok = expect(response_json.contains("core_lease"),
              "HelloResponse must include core_lease") &&
       ok;
  ok = expect(response_json.contains("task_queue"),
              "HelloResponse must include task_queue") &&
       ok;
  ok = expect(!json_has_key_recursive(response_json, "protocol_version"),
              "HelloResponse must not contain protocol_version") &&
       ok;
  ok = expect(!json_has_key_recursive(response_json, "server_version"),
              "HelloResponse must not contain server_version") &&
       ok;

  return ok ? 0 : 1;
}

int test_hello_mode_matches_startup_context() {
  bool ok = true;
  exv::helper::HelperHandler service_handler;

  exv::helper::HelperStartupContext service_context;
  service_context.launch_mode = "service";
  service_handler.set_startup_context(service_context);

  auto service_hello =
      dispatch_json(service_handler, exv::helper::HelperOp::Hello, json::object());
  ok = expect(service_hello.success, "service Hello should succeed") && ok;
  auto service_payload =
      exv::helper::hello_response_from_json(json::parse(service_hello.payload_json));
  ok = expect(service_payload.mode == exv::helper::HelperMode::Resident,
              "service Hello should report Resident mode") &&
       ok;

  exv::helper::HelperHandler oneshot_handler;
  exv::helper::HelperStartupContext oneshot_context;
  oneshot_context.launch_mode = "oneshot";
  oneshot_handler.set_startup_context(oneshot_context);

  auto oneshot_hello =
      dispatch_json(oneshot_handler, exv::helper::HelperOp::Hello, json::object());
  ok = expect(oneshot_hello.success, "oneshot Hello should succeed") && ok;
  auto oneshot_payload =
      exv::helper::hello_response_from_json(json::parse(oneshot_hello.payload_json));
  ok = expect(oneshot_payload.mode == exv::helper::HelperMode::Transient,
              "oneshot Hello should report Transient mode") &&
       ok;

  return ok ? 0 : 1;
}

int test_start_session_rejects_second_active_session() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  auto acquired = acquire_core_lease(handler);
  ok = expect(acquired.accepted,
              "core lease should be acquired before StartSession") &&
       ok;

  exv::helper::StartSessionRequest first_req;
  first_req.profile_id.value = "profile-a";
  auto first = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                             json(first_req));
  ok = expect(first.success, "first StartSession should succeed") && ok;

  exv::helper::StartSessionRequest second_req;
  second_req.profile_id.value = "profile-b";
  auto second = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                              json(second_req));
  ok = expect(!second.success, "second StartSession must be rejected") && ok;
  ok = expect(second.error_code == "session_conflict",
              "duplicate StartSession must return session_conflict") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 1,
              "handler must keep only one active session") &&
       ok;

  return ok ? 0 : 1;
}

int test_start_session_requires_core_lease() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "profile-a";
  auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                             json(start_req));

  ok = expect(!start.success,
              "StartSession before AcquireCoreLease must be rejected") &&
       ok;
  ok = expect(start.error_code == "core_lease_required",
              "StartSession without core lease must return core_lease_required") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 0,
              "rejected StartSession must not create a session") &&
       ok;

  return ok ? 0 : 1;
}

int test_start_session_rejects_supervisor_start_modes() {
  bool ok = true;

  for (const std::string &mode : {"password", "auth_session"}) {
    exv::helper::HelperHandler handler;
    auto acquired = acquire_core_lease(handler);
    ok = expect(acquired.accepted,
                "core lease should be acquired before legacy start rejection") &&
         ok;

    json payload = {
        {"profile_id", "profile-a"},
        {"native_start_mode", mode},
    };
    auto start =
        dispatch_json(handler, exv::helper::HelperOp::StartSession, payload);

    ok = expect(!start.success,
                "StartSession must reject legacy supervisor start modes") &&
         ok;
    ok = expect(start.error_code == "supervisor_removed",
                "legacy supervisor start mode must return supervisor_removed") &&
         ok;
    ok = expect(handler.lease_manager().active_session_count() == 0,
                "rejected supervisor start mode must not create a helper session") &&
         ok;
  }

  return ok ? 0 : 1;
}

int test_acquire_core_lease_allows_start_session() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  auto acquired = acquire_core_lease(handler, 4321, "connect");
  ok = expect(acquired.accepted, "valid AcquireCoreLease should be accepted") &&
       ok;
  ok = expect(!acquired.lease_id.empty(),
              "accepted AcquireCoreLease should return a lease_id") &&
       ok;

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "profile-a";
  auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                             json(start_req));
  ok = expect(start.success, "StartSession after AcquireCoreLease should succeed") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 1,
              "StartSession after core lease should create a session") &&
       ok;

  return ok ? 0 : 1;
}

int test_core_lease_rejects_invalid_and_conflicting_acquire() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  exv::helper::AcquireCoreLeaseRequest invalid_req;
  invalid_req.core_pid = 0;
  invalid_req.purpose = "connect";
  auto invalid = dispatch_json(handler, exv::helper::HelperOp::AcquireCoreLease,
                               json(invalid_req));
  ok = expect(!invalid.success,
              "AcquireCoreLease with invalid core_pid must fail") &&
       ok;

  auto acquired = acquire_core_lease(handler, 4321, "connect");
  ok = expect(acquired.accepted, "first AcquireCoreLease should be accepted") &&
       ok;

  exv::helper::AcquireCoreLeaseRequest conflict_req;
  conflict_req.core_pid = 9876;
  conflict_req.purpose = "connect";
  auto conflict = dispatch_json(handler, exv::helper::HelperOp::AcquireCoreLease,
                                json(conflict_req));
  ok = expect(!conflict.success,
              "second active core lease acquisition must fail") &&
       ok;
  ok = expect(conflict.error_code == "core_lease_conflict",
              "conflicting core lease must report core_lease_conflict") &&
       ok;

  return ok ? 0 : 1;
}

int test_core_lease_requires_verified_bound_peer() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  exv::helper::AcquireCoreLeaseRequest acquire_req;
  acquire_req.core_pid = 4321;
  acquire_req.purpose = "connect";
  auto unverified = dispatch_json(
      handler, exv::helper::HelperOp::AcquireCoreLease, json(acquire_req),
      unverified_context());
  ok = expect(!unverified.success,
              "AcquireCoreLease must reject unverified peers") &&
       ok;
  ok = expect(unverified.error_code == "core_lease_unauthorized",
              "unverified AcquireCoreLease must report unauthorized") &&
       ok;

  auto owner = peer_context("owner-a", 4321, 1001);
  acquire_req.core_pid = 9999;
  auto wrong_pid = dispatch_json(
      handler, exv::helper::HelperOp::AcquireCoreLease, json(acquire_req),
      owner);
  ok = expect(!wrong_pid.success,
              "AcquireCoreLease must reject mismatched peer pid") &&
       ok;
  ok = expect(wrong_pid.error_code == "core_lease_unauthorized",
              "pid mismatch must report unauthorized") &&
       ok;

  auto acquired = acquire_core_lease(handler, owner, 4321, "connect");
  ok = expect(acquired.accepted,
              "verified owner peer should acquire the CoreLease") &&
       ok;

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "profile-a";
  auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                             json(start_req), owner);
  ok = expect(start.success,
              "bound owner peer should use the active CoreLease") &&
       ok;

  auto other = peer_context("owner-b", 5555, 1002);
  start_req.profile_id.value = "profile-b";
  auto borrowed_start = dispatch_json(
      handler, exv::helper::HelperOp::StartSession, json(start_req), other);
  ok = expect(!borrowed_start.success,
              "different peer must not borrow an active CoreLease") &&
       ok;
  ok = expect(borrowed_start.error_code == "core_lease_unauthorized",
              "borrowed CoreLease request must report unauthorized") &&
       ok;

  exv::helper::KeepAliveRequest keep_alive_req;
  keep_alive_req.lease_id = acquired.lease_id;
  keep_alive_req.state = "connected";
  auto borrowed_keep_alive = dispatch_json(
      handler, exv::helper::HelperOp::KeepAlive, json(keep_alive_req), other);
  ok = expect(!borrowed_keep_alive.success,
              "different peer must not keep alive another CoreLease") &&
       ok;

  exv::helper::ReleaseCoreLeaseRequest release_req;
  release_req.lease_id = acquired.lease_id;
  auto borrowed_release = dispatch_json(
      handler, exv::helper::HelperOp::ReleaseCoreLease, json(release_req),
      other);
  ok = expect(!borrowed_release.success,
              "different peer must not release another CoreLease") &&
       ok;

  auto inspect = dispatch_json(handler, exv::helper::HelperOp::Inspect,
                               json(exv::helper::InspectRequest{}), other);
  ok = expect(inspect.success, "Inspect by other peer should still respond") &&
       ok;
  auto inspect_resp = exv::helper::inspect_response_from_json(
      json::parse(inspect.payload_json));
  ok = expect(inspect_resp.core_lease.active,
              "redacted Inspect should reveal only that a lease exists") &&
       ok;
  ok = expect(inspect_resp.core_lease.lease_id.empty() &&
                  inspect_resp.core_lease.core_pid == 0 &&
                  inspect_resp.core_lease.purpose.empty(),
              "Inspect must redact CoreLease details from other peers") &&
       ok;
  ok = expect(inspect_resp.session_state.active &&
                  inspect_resp.session_state.session_id.value.empty(),
              "Inspect must redact session id from other peers") &&
       ok;

  return ok ? 0 : 1;
}

int test_expired_core_lease_does_not_authorize_before_tick() {
  bool ok = true;
  exv::helper::LeaseTimeoutConfig config;
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy(config, std::chrono::seconds(0))};
  auto owner = peer_context("owner-a", 4321, 1001);

  auto acquired = acquire_core_lease(handler, owner, 4321, "connect");
  ok = expect(acquired.accepted, "CoreLease should acquire before expiry test") &&
       ok;

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "profile-a";
  auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                             json(start_req), owner);
  ok = expect(!start.success,
              "expired CoreLease must not authorize StartSession before tick") &&
       ok;
  ok = expect(start.error_code == "core_lease_required",
              "expired CoreLease request must report missing lease after cleanup") &&
       ok;
  ok = expect(!handler.has_active_core_lease(),
              "expired CoreLease must be cleared during request handling") &&
       ok;

  return ok ? 0 : 1;
}

int test_keepalive_requires_matching_core_lease() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  exv::helper::KeepAliveRequest invalid_req;
  invalid_req.lease_id = "missing";
  invalid_req.state = "connected";
  auto invalid = dispatch_json(handler, exv::helper::HelperOp::KeepAlive,
                               json(invalid_req));
  ok = expect(!invalid.success,
              "KeepAlive without matching lease must fail") &&
       ok;
  ok = expect(invalid.error_code == "invalid_core_lease",
              "invalid KeepAlive must report invalid_core_lease") &&
       ok;

  auto acquired = acquire_core_lease(handler);
  exv::helper::KeepAliveRequest keep_alive_req;
  keep_alive_req.lease_id = acquired.lease_id;
  keep_alive_req.state = "connected";
  auto keep_alive = dispatch_json(handler, exv::helper::HelperOp::KeepAlive,
                                  json(keep_alive_req));
  ok = expect(keep_alive.success, "matching KeepAlive should succeed") && ok;

  auto inspect = dispatch_json(handler, exv::helper::HelperOp::Inspect,
                               json(exv::helper::InspectRequest{}));
  ok = expect(inspect.success, "Inspect after KeepAlive should succeed") && ok;
  auto inspect_resp = exv::helper::inspect_response_from_json(
      json::parse(inspect.payload_json));
  ok = expect(inspect_resp.core_lease.last_seen_state == "connected",
              "KeepAlive should update core lease last_seen_state") &&
       ok;

  return ok ? 0 : 1;
}

int test_inspect_reports_active_core_lease_and_session_state() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "session should start before Inspect") &&
       ok;

  auto inspect = dispatch_json(handler, exv::helper::HelperOp::Inspect,
                               json(exv::helper::InspectRequest{}));
  ok = expect(inspect.success, "Inspect should succeed") && ok;
  auto inspect_resp = exv::helper::inspect_response_from_json(
      json::parse(inspect.payload_json));
  ok = expect(inspect_resp.core_lease.active,
              "Inspect must report active core lease") &&
       ok;
  ok = expect(inspect_resp.core_lease.core_pid == 4321,
              "Inspect must report core lease pid") &&
       ok;
  ok = expect(inspect_resp.core_lease.purpose == "connect",
              "Inspect must report core lease purpose") &&
       ok;
  ok = expect(inspect_resp.session_state.active,
              "Inspect must report active VPN session") &&
       ok;
  ok = expect(inspect_resp.session_state.session_id == start_resp.session_id,
              "Inspect must report active session id") &&
       ok;

  return ok ? 0 : 1;
}

int test_shutdown_cleans_active_session() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(), "StartSession should succeed") &&
       ok;

  exv::helper::ShutdownRequest shutdown_req;
  shutdown_req.session_id = start_resp.session_id;
  shutdown_req.policy.remove_adapter = true;
  auto shutdown = dispatch_json(handler, exv::helper::HelperOp::Shutdown,
                                json(shutdown_req));
  ok = expect(shutdown.success, "Shutdown should succeed") && ok;
  ok = expect(handler.lease_manager().active_session_count() == 0,
              "Shutdown must remove the active session") &&
       ok;

  const auto shutdown_resp =
      exv::helper::shutdown_response_from_json(json::parse(shutdown.payload_json));
  ok = expect(shutdown_resp.cleanup_success,
              "Shutdown response must report cleanup success") &&
       ok;

  return ok ? 0 : 1;
}

int test_oneshot_shutdown_cleans_session_without_exiting_core_lease() {
  bool ok = true;
  exv::helper::HelperHandler handler;
  exv::helper::HelperStartupContext context;
  context.launch_mode = "oneshot";
  handler.set_startup_context(context);

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "oneshot StartSession should succeed after core lease") &&
       ok;

  exv::helper::ShutdownRequest shutdown_req;
  shutdown_req.session_id = start_resp.session_id;
  shutdown_req.policy.remove_adapter = true;
  auto shutdown = dispatch_json(handler, exv::helper::HelperOp::Shutdown,
                                json(shutdown_req));
  ok = expect(shutdown.success, "oneshot Shutdown should succeed") && ok;
  auto shutdown_resp =
      exv::helper::shutdown_response_from_json(json::parse(shutdown.payload_json));
  ok = expect(shutdown_resp.cleanup_success,
              "oneshot Shutdown should report cleanup success") &&
       ok;
  ok = expect(!shutdown_resp.exiting,
              "oneshot Shutdown must not report exiting while core lease is active") &&
       ok;
  ok = expect(!handler.should_stop(),
              "oneshot Shutdown must not request helper exit while core lease is active") &&
       ok;

  auto inspect = dispatch_json(handler, exv::helper::HelperOp::Inspect,
                               json(exv::helper::InspectRequest{}));
  auto inspect_resp = exv::helper::inspect_response_from_json(
      json::parse(inspect.payload_json));
  ok = expect(inspect_resp.core_lease.active,
              "core lease should remain active after VPN Shutdown") &&
       ok;
  ok = expect(!inspect_resp.session_state.active,
              "VPN session should be inactive after Shutdown") &&
       ok;

  return ok ? 0 : 1;
}

int test_release_core_lease_requests_oneshot_exit() {
  bool ok = true;
  exv::helper::HelperHandler handler;
  exv::helper::HelperStartupContext context;
  context.launch_mode = "oneshot";
  handler.set_startup_context(context);

  auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "oneshot session should start before releasing core lease") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 1,
              "oneshot should have an active session before core lease release") &&
       ok;
  auto active_lease = handler.has_active_core_lease();
  ok = expect(active_lease, "oneshot core lease should be active") && ok;
  auto lease = handler.active_core_pid();
  (void)lease;

  exv::helper::ReleaseCoreLeaseRequest release_req;
  auto inspect = dispatch_json(handler, exv::helper::HelperOp::Inspect,
                               json(exv::helper::InspectRequest{}));
  auto inspect_resp = exv::helper::inspect_response_from_json(
      json::parse(inspect.payload_json));
  release_req.lease_id = inspect_resp.core_lease.lease_id;
  release_req.exit_if_oneshot = true;
  auto release = dispatch_json(handler, exv::helper::HelperOp::ReleaseCoreLease,
                               json(release_req));
  ok = expect(release.success, "ReleaseCoreLease should succeed") && ok;
  auto release_resp = exv::helper::release_core_lease_response_from_json(
      json::parse(release.payload_json));
  ok = expect(release_resp.released,
              "ReleaseCoreLease response should report released") &&
       ok;
  ok = expect(release_resp.exiting,
              "ReleaseCoreLease should report exiting for oneshot") &&
       ok;
  ok = expect(handler.should_stop(),
              "ReleaseCoreLease should request oneshot helper exit") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 0,
              "ReleaseCoreLease must cleanup active sessions before oneshot exit") &&
       ok;

  return ok ? 0 : 1;
}

int test_cleanup_retains_resources_when_platform_cleanup_unavailable() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "StartSession should succeed before cleanup test") &&
       ok;

  handler.cleanup_registry().add_resource(start_resp.session_id,
                                          {"route", "0.0.0.0/0"});

  exv::helper::CleanupRequest cleanup_req;
  cleanup_req.session_id = start_resp.session_id;
  auto cleanup = dispatch_json(handler, exv::helper::HelperOp::Cleanup,
                               json(cleanup_req));
  ok = expect(!cleanup.success,
              "Cleanup must fail when managed resources cannot be cleaned") &&
       ok;
  ok = expect(cleanup.error_code == "cleanup_partial",
              "Cleanup failure should report cleanup_partial") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 1,
              "failed cleanup must keep the active session for retry") &&
       ok;
  ok = expect(handler.cleanup_registry().all_records().size() == 1,
              "failed cleanup must keep registry records for retry") &&
       ok;

  return ok ? 0 : 1;
}

int test_handler_delegates_network_ops_and_cleans_registered_resources() {
  bool ok = true;
  auto network_ops = std::make_shared<RecordingHelperNetworkOps>();
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy{}, network_ops};

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "StartSession should succeed before network ops") &&
       ok;

  exv::helper::PrepareTunnelDeviceRequest prepare_req;
  prepare_req.session_id = start_resp.session_id;
  prepare_req.adapter_name = "ECNU-VPN";
  auto prepare = dispatch_json(handler,
                               exv::helper::HelperOp::PrepareTunnelDevice,
                               json(prepare_req));
  ok = expect(prepare.success, "PrepareTunnelDevice should use network ops") &&
       ok;
  const auto prepare_resp = exv::helper::prepare_tunnel_device_response_from_json(
      json::parse(prepare.payload_json));
  ok = expect(prepare_resp.device_path == "helper-device://ECNU-VPN",
              "PrepareTunnelDevice should return network ops device path") &&
       ok;

  exv::helper::ApplyTunnelConfigRequest apply_req;
  apply_req.config.session_id = start_resp.session_id;
  apply_req.config.interface_address = "10.0.0.2/24";
  apply_req.config.routes.push_back({"10.0.0.0/8", "10.0.0.1", 10});
  apply_req.config.dns.servers = {"10.0.0.53"};
  auto apply = dispatch_json(handler, exv::helper::HelperOp::ApplyTunnelConfig,
                             json(apply_req));
  ok = expect(apply.success, "ApplyTunnelConfig should use network ops") && ok;

  auto resources = handler.cleanup_registry().get_resources(start_resp.session_id);
  ok = expect(resources.size() == 3,
              "Prepare/Apply should register adapter, route, and DNS resources") &&
       ok;

  exv::helper::CleanupRequest cleanup_req;
  cleanup_req.session_id = start_resp.session_id;
  auto cleanup = dispatch_json(handler, exv::helper::HelperOp::Cleanup,
                               json(cleanup_req));
  ok = expect(cleanup.success, "Cleanup should use network ops") && ok;
  ok = expect(handler.lease_manager().active_session_count() == 0,
              "successful cleanup should remove the active lease") &&
       ok;
  ok = expect(handler.cleanup_registry().all_records().empty(),
              "successful cleanup should remove registry records") &&
       ok;
  ok = expect(network_ops->prepare_count == 1,
              "network ops prepare should be called once") &&
       ok;
  ok = expect(network_ops->apply_count == 1,
              "network ops apply should be called once") &&
       ok;
  ok = expect(network_ops->cleanup_count == 1,
              "network ops cleanup should be called once") &&
       ok;
  ok = expect(network_ops->last_cleanup_resource_count == 3,
              "network ops cleanup should receive all tracked resources") &&
       ok;

  return ok ? 0 : 1;
}

int test_privileged_task_queue_serializes_mutations_and_reports_state() {
  bool ok = true;
  auto network_ops = std::make_shared<BlockingHelperNetworkOps>();
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy{}, network_ops};

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "StartSession should succeed before queued mutations") &&
       ok;

  exv::helper::PrepareTunnelDeviceRequest prepare_req;
  prepare_req.session_id = start_resp.session_id;
  prepare_req.adapter_name = "ECNU-VPN";

  exv::helper::HelperResponse prepare_resp;
  std::thread prepare_thread([&] {
    prepare_resp = dispatch_json(handler,
                                 exv::helper::HelperOp::PrepareTunnelDevice,
                                 json(prepare_req));
  });

  ok = expect(network_ops->wait_for_prepare_started(
                  std::chrono::milliseconds(500)),
              "PrepareTunnelDevice should start on the privileged queue") &&
       ok;

  exv::helper::ApplyTunnelConfigRequest apply_req;
  apply_req.config.session_id = start_resp.session_id;
  apply_req.config.interface_address = "10.0.0.2/24";
  apply_req.config.routes.push_back({"10.0.0.0/8", "10.0.0.1", 10});

  exv::helper::HelperResponse apply_resp;
  std::thread apply_thread([&] {
    apply_resp = dispatch_json(handler,
                               exv::helper::HelperOp::ApplyTunnelConfig,
                               json(apply_req));
  });

  bool saw_pending = wait_until([&] {
    auto inspect = dispatch_json(handler, exv::helper::HelperOp::Inspect,
                                 json(exv::helper::InspectRequest{}));
    if (!inspect.success) {
      return false;
    }
    auto inspect_resp = exv::helper::inspect_response_from_json(
        json::parse(inspect.payload_json));
    return !inspect_resp.task_queue.idle &&
           !inspect_resp.task_queue.current_job_id.empty() &&
           inspect_resp.task_queue.pending_jobs >= 1;
  }, std::chrono::milliseconds(500));
  ok = expect(saw_pending,
              "Inspect must expose a busy task queue with a pending mutation") &&
       ok;

  network_ops->release_prepare();
  if (prepare_thread.joinable()) {
    prepare_thread.join();
  }
  if (apply_thread.joinable()) {
    apply_thread.join();
  }

  ok = expect(prepare_resp.success,
              "queued PrepareTunnelDevice should complete successfully") &&
       ok;
  ok = expect(apply_resp.success,
              "queued ApplyTunnelConfig should complete successfully") &&
       ok;
  ok = expect(network_ops->max_active() == 1,
              "privileged network mutations must not overlap") &&
       ok;
  const auto calls = network_ops->calls();
  ok = expect(calls.size() == 2 && calls[0] == "prepare" &&
                  calls[1] == "apply",
              "queued mutations should run in submission order") &&
       ok;

  auto inspect = dispatch_json(handler, exv::helper::HelperOp::Inspect,
                               json(exv::helper::InspectRequest{}));
  auto inspect_resp = exv::helper::inspect_response_from_json(
      json::parse(inspect.payload_json));
  ok = expect(inspect_resp.task_queue.idle,
              "task queue should return to idle after queued mutations finish") &&
       ok;

  return ok ? 0 : 1;
}

int test_core_lifecycle_cleanup_reports_task_queue_state() {
  bool ok = true;
  auto network_ops = std::make_shared<BlockingHelperNetworkOps>();
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy{}, network_ops};

  exv::helper::HelperStartupContext context;
  context.launch_mode = "oneshot";
  handler.set_startup_context(context);

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "StartSession should succeed before lifecycle cleanup") &&
       ok;
  handler.cleanup_registry().add_resource(start_resp.session_id,
                                          {"adapter", "ECNU-VPN"});

  std::thread cleanup_thread([&] { handler.handle_core_lifecycle_lost(); });

  ok = expect(network_ops->wait_for_cleanup_started(
                  std::chrono::milliseconds(500)),
              "core-lifecycle cleanup should start on the privileged queue") &&
       ok;

  auto inspect = dispatch_json(handler, exv::helper::HelperOp::Inspect,
                               json(exv::helper::InspectRequest{}));
  ok = expect(inspect.success, "Inspect during cleanup should succeed") && ok;
  auto inspect_resp = exv::helper::inspect_response_from_json(
      json::parse(inspect.payload_json));
  ok = expect(!inspect_resp.task_queue.idle,
              "Inspect must show task queue busy during lifecycle cleanup") &&
       ok;
  ok = expect(!inspect_resp.task_queue.current_job_id.empty(),
              "busy lifecycle cleanup must expose current job id") &&
       ok;

  network_ops->release_cleanup();
  if (cleanup_thread.joinable()) {
    cleanup_thread.join();
  }

  ok = expect(handler.lease_manager().active_session_count() == 0,
              "core-lifecycle cleanup should remove active session") &&
       ok;
  ok = expect(handler.should_stop(),
              "oneshot core-lifecycle cleanup should request helper exit") &&
       ok;
  ok = expect(network_ops->max_active() == 1,
              "core-lifecycle cleanup must run without overlapping mutations") &&
       ok;

  return ok ? 0 : 1;
}

int test_busy_privileged_task_keeps_core_lease_alive() {
  bool ok = true;
  auto network_ops = std::make_shared<BlockingHelperNetworkOps>();
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy({}, std::chrono::seconds(1)),
      network_ops};

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "StartSession should succeed before busy task test") &&
       ok;

  exv::helper::PrepareTunnelDeviceRequest prepare_req;
  prepare_req.session_id = start_resp.session_id;
  prepare_req.adapter_name = "ECNU-VPN";

  exv::helper::HelperResponse prepare_resp;
  std::thread prepare_thread([&] {
    prepare_resp = dispatch_json(handler,
                                 exv::helper::HelperOp::PrepareTunnelDevice,
                                 json(prepare_req));
  });

  ok = expect(network_ops->wait_for_prepare_started(
                  std::chrono::milliseconds(500)),
              "PrepareTunnelDevice should be running before timeout tick") &&
       ok;

  std::this_thread::sleep_for(std::chrono::milliseconds(1100));
  handler.tick();
  ok = expect(handler.has_active_core_lease(),
              "busy privileged task must keep core lease alive") &&
       ok;
  ok = expect(!handler.should_stop(),
              "oneshot/service should not stop while a privileged task is busy") &&
       ok;

  network_ops->release_prepare();
  if (prepare_thread.joinable()) {
    prepare_thread.join();
  }

  ok = expect(prepare_resp.success,
              "busy PrepareTunnelDevice should complete successfully") &&
       ok;
  handler.tick();
  ok = expect(handler.has_active_core_lease(),
              "completed privileged task should refresh core lease activity") &&
       ok;

  return ok ? 0 : 1;
}

int test_cleanup_all_sessions_reports_failure_and_keeps_retry_state() {
  bool ok = true;
  auto network_ops = std::make_shared<RecordingHelperNetworkOps>();
  network_ops->cleanup_success = false;
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy{}, network_ops};

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "StartSession should succeed before failed cleanup") &&
       ok;

  handler.cleanup_registry().add_resource(start_resp.session_id,
                                          {"adapter", "ECNU-VPN"});

  exv::helper::CleanupPolicy policy;
  policy.remove_adapter = true;
  auto cleanup = handler.cleanup_all_sessions(policy);
  ok = expect(!cleanup.success,
              "cleanup_all_sessions must report managed cleanup failure") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 1,
              "failed cleanup_all_sessions must keep lease for retry") &&
       ok;
  ok = expect(handler.cleanup_registry().all_records().size() == 1,
              "failed cleanup_all_sessions must keep registry for retry") &&
       ok;

  return ok ? 0 : 1;
}

int test_network_ops_do_not_report_fake_success() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "StartSession should succeed") &&
       ok;

  exv::helper::PrepareTunnelDeviceRequest prepare_req;
  prepare_req.session_id = start_resp.session_id;
  prepare_req.adapter_name = "ECNU-VPN";
  auto prepare = dispatch_json(handler, exv::helper::HelperOp::PrepareTunnelDevice,
                               json(prepare_req));
  ok = expect(!prepare.success,
              "PrepareTunnelDevice must not fake platform success") &&
       ok;
  ok = expect(prepare.error_code == "network_ops_unavailable",
              "PrepareTunnelDevice must report network_ops_unavailable") &&
       ok;

  exv::helper::ApplyTunnelConfigRequest apply_req;
  apply_req.config.session_id = start_resp.session_id;
  apply_req.config.interface_address = "10.0.0.2/24";
  auto apply = dispatch_json(handler, exv::helper::HelperOp::ApplyTunnelConfig,
                             json(apply_req));
  ok = expect(!apply.success,
              "ApplyTunnelConfig must not fake platform success") &&
       ok;
  ok = expect(apply.error_code == "network_ops_unavailable",
              "ApplyTunnelConfig must report network_ops_unavailable") &&
       ok;

  return ok ? 0 : 1;
}

int test_service_maintenance_requires_core_lease_without_vpn_session() {
  bool ok = true;
  auto service_ops = std::make_shared<RecordingHelperServiceOps>();
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy{}, nullptr, service_ops};

  auto without_lease = dispatch_json(
      handler, exv::helper::HelperOp::InstallService,
      json(exv::helper::InstallServiceRequest{}));
  ok = expect(!without_lease.success,
              "InstallService must require an active core lease") &&
       ok;
  ok = expect(without_lease.error_code == "core_lease_required",
              "InstallService without lease must report core_lease_required") &&
       ok;
  ok = expect(service_ops->install_count == 0,
              "InstallService must not call platform ops without a lease") &&
       ok;

  const auto lease = acquire_core_lease(handler);
  ok = expect(lease.accepted, "core lease should be acquired") && ok;

  auto install = dispatch_json(handler, exv::helper::HelperOp::InstallService,
                               json(exv::helper::InstallServiceRequest{}));
  ok = expect(install.success,
              "InstallService should succeed with core lease and no VPN session") &&
       ok;
  auto install_resp = exv::helper::install_service_response_from_json(
      json::parse(install.payload_json));
  ok = expect(install_resp.success && install_resp.exit_code == 0,
              "InstallService response should preserve platform result") &&
       ok;

  auto uninstall =
      dispatch_json(handler, exv::helper::HelperOp::UninstallService,
                    json(exv::helper::UninstallServiceRequest{}));
  ok = expect(uninstall.success,
              "UninstallService should succeed with core lease and no VPN session") &&
       ok;
  auto uninstall_resp = exv::helper::uninstall_service_response_from_json(
      json::parse(uninstall.payload_json));
  ok = expect(uninstall_resp.success && uninstall_resp.exit_code == 0,
              "UninstallService response should preserve platform result") &&
       ok;
  ok = expect(service_ops->install_count == 1,
              "InstallService should call service ops exactly once") &&
       ok;
  ok = expect(service_ops->uninstall_count == 1,
              "UninstallService should call service ops exactly once") &&
       ok;

  return ok ? 0 : 1;
}

int test_service_uninstall_rejects_active_vpn_session_in_helper() {
  bool ok = true;
  auto network_ops = std::make_shared<RecordingHelperNetworkOps>();
  auto service_ops = std::make_shared<RecordingHelperServiceOps>();
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy{}, network_ops, service_ops};

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "helper should have an active VPN session") &&
       ok;

  auto uninstall =
      dispatch_json(handler, exv::helper::HelperOp::UninstallService,
                    json(exv::helper::UninstallServiceRequest{}));
  ok = expect(!uninstall.success,
              "UninstallService must reject an active VPN session") &&
       ok;
  ok = expect(uninstall.error_code == "vpn_session_active",
              "UninstallService active-session rejection should use vpn_session_active") &&
       ok;
  ok = expect(service_ops->uninstall_count == 0,
              "UninstallService must not call platform service ops while VPN is active") &&
       ok;

  return ok ? 0 : 1;
}

int test_cleanup_lease_handoff_moves_session_to_service() {
  bool ok = true;
  auto oneshot_ops = std::make_shared<RecordingHelperNetworkOps>();
  exv::helper::HelperHandler oneshot{
      exv::helper::HelperLifecyclePolicy{}, oneshot_ops};
  exv::helper::HelperStartupContext oneshot_context;
  oneshot_context.launch_mode = "oneshot";
  oneshot.set_startup_context(oneshot_context);

  const auto start_resp = start_session_with_core_lease(oneshot);
  ok = expect(!start_resp.session_id.value.empty(),
              "oneshot should start a session before export") &&
       ok;
  oneshot.cleanup_registry().add_resource(start_resp.session_id,
                                          {"adapter", "ECNU-VPN"});
  oneshot.cleanup_registry().add_resource(start_resp.session_id,
                                          {"route", "10.0.0.0/8"});

  auto exported = dispatch_json(
      oneshot, exv::helper::HelperOp::ExportCleanupLease,
      json(exv::helper::ExportCleanupLeaseRequest{}));
  ok = expect(exported.success,
              "ExportCleanupLease should succeed with active core lease") &&
       ok;
  auto export_resp = exv::helper::export_cleanup_lease_response_from_json(
      json::parse(exported.payload_json));
  ok = expect(export_resp.has_active_session,
              "exported cleanup lease should report active session") &&
       ok;
  ok = expect(export_resp.lease.sessions.size() == 1,
              "exported cleanup lease should contain one session") &&
       ok;
  ok = expect(export_resp.lease.sessions[0].managed_resources.size() == 2,
              "exported cleanup lease should contain actual managed resources") &&
       ok;

  auto service_ops = std::make_shared<RecordingHelperNetworkOps>();
  exv::helper::HelperHandler service{
      exv::helper::HelperLifecyclePolicy{}, service_ops};
  exv::helper::HelperStartupContext service_context;
  service_context.launch_mode = "service";
  service.set_startup_context(service_context);
  const auto service_lease =
      acquire_core_lease(service, 7777, "service.handoff");
  ok = expect(service_lease.accepted,
              "service should acquire a core lease before handoff") &&
       ok;

  exv::helper::HandoffSessionRequest handoff_req;
  handoff_req.lease = export_resp.lease;
  auto handoff = dispatch_json(service, exv::helper::HelperOp::HandoffSession,
                               json(handoff_req));
  ok = expect(handoff.success, "service should adopt cleanup lease") && ok;
  auto handoff_resp = exv::helper::handoff_session_response_from_json(
      json::parse(handoff.payload_json));
  ok = expect(handoff_resp.adopted,
              "handoff response should report adopted") &&
       ok;
  ok = expect(service.lease_manager().has_session(start_resp.session_id),
              "service should now own imported session") &&
       ok;
  ok = expect(service.cleanup_registry().get_resources(start_resp.session_id).size() ==
                  2,
              "service should import cleanup resources") &&
       ok;

  auto finalize = dispatch_json(
      oneshot, exv::helper::HelperOp::FinalizeHandoff,
      json(exv::helper::FinalizeHandoffRequest{}));
  ok = expect(finalize.success, "oneshot should finalize handoff") && ok;
  auto finalize_resp = exv::helper::finalize_handoff_response_from_json(
      json::parse(finalize.payload_json));
  ok = expect(finalize_resp.finalized && finalize_resp.exiting,
              "oneshot finalize should request exit by default") &&
       ok;
  ok = expect(oneshot.lease_manager().active_session_count() == 0,
              "oneshot should drop local session after finalize") &&
       ok;
  ok = expect(oneshot.cleanup_registry().get_resources(start_resp.session_id).empty(),
              "oneshot should drop cleanup registry after finalize") &&
       ok;
  ok = expect(oneshot.should_stop(),
              "oneshot finalize with exit=true should request daemon stop") &&
       ok;

  return ok ? 0 : 1;
}

int test_session_heartbeat_timeout_cleans_but_keeps_oneshot_with_core_lease() {
  bool ok = true;
  exv::helper::LeaseTimeoutConfig config;
  config.transient_heartbeat_timeout = std::chrono::seconds(0);
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy(config)};

  exv::helper::HelperStartupContext context;
  context.launch_mode = "oneshot";
  handler.set_startup_context(context);

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "oneshot StartSession should succeed") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 1,
              "oneshot should have an active session before timeout") &&
       ok;

  handler.tick();
  ok = expect(handler.lease_manager().active_session_count() == 0,
              "oneshot timeout tick must clean the active session") &&
       ok;
  ok = expect(!handler.should_stop(),
              "oneshot session heartbeat timeout must not exit while core lease is active") &&
       ok;

  return ok ? 0 : 1;
}

int test_core_lease_timeout_cleans_session_and_exits_in_oneshot() {
  bool ok = true;
  exv::helper::LeaseTimeoutConfig config;
  config.transient_heartbeat_timeout = std::chrono::seconds(60);
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy(config, std::chrono::seconds(1))};

  exv::helper::HelperStartupContext context;
  context.launch_mode = "oneshot";
  handler.set_startup_context(context);

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "oneshot StartSession should succeed before core lease timeout") &&
       ok;

  std::this_thread::sleep_for(std::chrono::milliseconds(1100));
  handler.tick();
  ok = expect(handler.lease_manager().active_session_count() == 0,
              "core lease timeout must clean the active session") &&
       ok;
  ok = expect(handler.should_stop(),
              "core lease timeout must request oneshot helper exit") &&
       ok;

  return ok ? 0 : 1;
}

int test_core_lease_timeout_cleans_without_exit_in_service() {
  bool ok = true;
  exv::helper::LeaseTimeoutConfig config;
  config.transient_heartbeat_timeout = std::chrono::seconds(60);
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy(config, std::chrono::seconds(1))};

  exv::helper::HelperStartupContext context;
  context.launch_mode = "service";
  handler.set_startup_context(context);

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "service StartSession should succeed before core lease timeout") &&
       ok;

  std::this_thread::sleep_for(std::chrono::milliseconds(1100));
  handler.tick();
  ok = expect(handler.lease_manager().active_session_count() == 0,
              "service core lease timeout must clean the active session") &&
       ok;
  ok = expect(!handler.should_stop(),
              "service core lease timeout must keep daemon running") &&
       ok;

  return ok ? 0 : 1;
}

int test_helper_message_fields_are_credential_free() {
  bool ok = true;

  auto check = [&](const json &message, const char *name) {
    if (json_contains_credential_field(message)) {
      std::cerr << "EXPECT FAILED: " << name
                << " contains a credential-like field\n";
      ok = false;
    }
  };

  check(json(exv::helper::HelloRequest{}), "HelloRequest");
  check(json(exv::helper::HelloResponse{}), "HelloResponse");

  exv::helper::StartSessionRequest start;
  start.profile_id.value = "profile";
  check(json(start), "StartSessionRequest");

  exv::helper::PrepareTunnelDeviceRequest prepare;
  prepare.session_id.value = "ses-1";
  prepare.adapter_name = "adapter";
  check(json(prepare), "PrepareTunnelDeviceRequest");

  exv::helper::ApplyTunnelConfigRequest apply;
  apply.config.session_id.value = "ses-1";
  apply.config.interface_address = "10.0.0.2/24";
  apply.config.routes.push_back({"0.0.0.0/0", "10.0.0.1", 100});
  apply.config.dns.servers = {"8.8.8.8"};
  check(json(apply), "ApplyTunnelConfigRequest");

  exv::helper::HeartbeatRequest heartbeat;
  heartbeat.session_id.value = "ses-1";
  heartbeat.core_phase = "Connected";
  check(json(heartbeat), "HeartbeatRequest");

  exv::helper::CleanupRequest cleanup;
  cleanup.session_id.value = "ses-1";
  check(json(cleanup), "CleanupRequest");

  check(json(exv::helper::GetSnapshotRequest{}), "GetSnapshotRequest");

  exv::helper::ShutdownRequest shutdown;
  shutdown.session_id.value = "ses-1";
  check(json(shutdown), "ShutdownRequest");

  check(json(exv::helper::InspectRequest{}), "InspectRequest");
  check(json(exv::helper::InspectResponse{}), "InspectResponse");

  exv::helper::AcquireCoreLeaseRequest acquire;
  acquire.core_pid = 1234;
  acquire.purpose = "connect";
  check(json(acquire), "AcquireCoreLeaseRequest");

  exv::helper::AcquireCoreLeaseResponse acquired;
  acquired.accepted = true;
  acquired.lease_id = "lease-1";
  acquired.mode = "oneshot";
  check(json(acquired), "AcquireCoreLeaseResponse");

  exv::helper::KeepAliveRequest keep_alive;
  keep_alive.lease_id = "lease-1";
  keep_alive.state = "connected";
  check(json(keep_alive), "KeepAliveRequest");

  exv::helper::KeepAliveResponse keep_alive_resp;
  keep_alive_resp.warning = "stale-state";
  check(json(keep_alive_resp), "KeepAliveResponse");

  exv::helper::ReleaseCoreLeaseRequest release;
  release.lease_id = "lease-1";
  release.exit_if_oneshot = true;
  check(json(release), "ReleaseCoreLeaseRequest");

  exv::helper::ReleaseCoreLeaseResponse released;
  released.released = true;
  released.exiting = true;
  check(json(released), "ReleaseCoreLeaseResponse");

  check(json(exv::helper::InstallServiceRequest{}), "InstallServiceRequest");
  check(json(exv::helper::InstallServiceResponse{}), "InstallServiceResponse");
  check(json(exv::helper::UninstallServiceRequest{}),
        "UninstallServiceRequest");
  check(json(exv::helper::UninstallServiceResponse{}),
        "UninstallServiceResponse");
  check(json(exv::helper::ExportCleanupLeaseRequest{}),
        "ExportCleanupLeaseRequest");
  check(json(exv::helper::ExportCleanupLeaseResponse{}),
        "ExportCleanupLeaseResponse");
  check(json(exv::helper::HandoffSessionRequest{}),
        "HandoffSessionRequest");
  check(json(exv::helper::HandoffSessionResponse{}),
        "HandoffSessionResponse");
  check(json(exv::helper::FinalizeHandoffRequest{}),
        "FinalizeHandoffRequest");
  check(json(exv::helper::FinalizeHandoffResponse{}),
        "FinalizeHandoffResponse");

  return ok ? 0 : 1;
}

} // namespace

int main() {
  int failures = 0;

  std::cout << "=== Helper Contract Tests ===\n";
  failures += test_manifest_declares_single_helper_protocol();
  failures += test_generated_contract_matches_helper_manifest();
  failures += test_daemon_uses_network_ops_handler_factory();
  failures += test_daemon_does_not_hold_external_handler_lock();
  failures += test_legacy_helper_internal_header_removed();
  failures += test_unused_helper_server_stub_removed();
  failures += test_helper_platform_boundary_lives_under_platform();
  failures += test_oneshot_owner_is_uid_or_sid_not_pid_alias();
  failures += test_windows_helper_has_no_second_service_entrypoint();
  failures += test_oneshot_entrypoint_uses_only_endpoint_argument();
  failures += test_helper_connector_requires_explicit_endpoint_field();
  failures += test_hello_has_no_version_fields();
  failures += test_hello_mode_matches_startup_context();
  failures += test_start_session_requires_core_lease();
  failures += test_start_session_rejects_supervisor_start_modes();
  failures += test_acquire_core_lease_allows_start_session();
  failures += test_core_lease_rejects_invalid_and_conflicting_acquire();
  failures += test_core_lease_requires_verified_bound_peer();
  failures += test_expired_core_lease_does_not_authorize_before_tick();
  failures += test_keepalive_requires_matching_core_lease();
  failures += test_inspect_reports_active_core_lease_and_session_state();
  failures += test_start_session_rejects_second_active_session();
  failures += test_shutdown_cleans_active_session();
  failures += test_oneshot_shutdown_cleans_session_without_exiting_core_lease();
  failures += test_release_core_lease_requests_oneshot_exit();
  failures += test_cleanup_retains_resources_when_platform_cleanup_unavailable();
  failures += test_handler_delegates_network_ops_and_cleans_registered_resources();
  failures +=
      test_privileged_task_queue_serializes_mutations_and_reports_state();
  failures += test_core_lifecycle_cleanup_reports_task_queue_state();
  failures += test_busy_privileged_task_keeps_core_lease_alive();
  failures += test_cleanup_all_sessions_reports_failure_and_keeps_retry_state();
  failures += test_network_ops_do_not_report_fake_success();
  failures +=
      test_service_maintenance_requires_core_lease_without_vpn_session();
  failures += test_service_uninstall_rejects_active_vpn_session_in_helper();
  failures += test_cleanup_lease_handoff_moves_session_to_service();
  failures +=
      test_session_heartbeat_timeout_cleans_but_keeps_oneshot_with_core_lease();
  failures += test_core_lease_timeout_cleans_session_and_exits_in_oneshot();
  failures += test_core_lease_timeout_cleans_without_exit_in_service();
  failures += test_helper_message_fields_are_credential_free();

  if (failures == 0) {
    std::cout << "helper_contract_test: all tests passed\n";
  } else {
    std::cerr << "helper_contract_test: " << failures << " test(s) FAILED\n";
  }
  return failures == 0 ? 0 : 1;
}
