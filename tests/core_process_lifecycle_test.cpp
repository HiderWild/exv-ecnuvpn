// Core process lifecycle tests (Task E2)
//
// E2.1: Start/exit lifecycle — runs core_process_main() in a thread with
//        redirected stdin/stdout, feeds JSON-RPC requests, and verifies
//        responses and clean exit codes.
// E2.2: Crash recovery — feeds malformed / mixed input and verifies the
//        process survives without crashing and still handles valid requests.
//
// NOTE: E2.1d (SIGTERM test) MUST run last because it sets the process-global
//       g_stop_requested flag which is never reset.  All subsequent calls to
//       core_process_main() in the same process would exit immediately.

#include "core/core_process.hpp"
#include "core/lifecycle/core_paths.hpp"
#include "core/lifecycle/core_registry.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"
#include "core/tunnel_controller/reconnect_policy.hpp"
#include "core/pipe_ipc.hpp"
#include "contracts/generated/system_contract.hpp"
#include "cli/pipe_client.hpp"
#include "core/app_api/app_api.hpp"
#include "core/app_api/desktop_vpn_test_hooks.hpp"

#include <nlohmann/json.hpp>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <csignal>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <functional>
#include <future>
#include <iostream>
#include <memory>
#include <mutex>
#include <optional>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

#ifdef _WIN32
#include <direct.h>
#include <windows.h>
#else
#include <sys/stat.h>
#include <unistd.h>
#endif

using json = nlohmann::json;

#ifndef ECNUVPN_VERSION
#define ECNUVPN_VERSION "test"
#endif

namespace exv::core::lifecycle::testing {
using CoreRegistryWriteHook =
    std::function<std::optional<bool>(
        const exv::core::lifecycle::CoreRegistrySnapshot& snapshot,
        const std::string& registry_path)>;

void set_write_core_registry_hook(CoreRegistryWriteHook hook);
} // namespace exv::core::lifecycle::testing

namespace exv::core::testing {
using CoreRegistryPersistCandidateHook =
    std::function<void(
        exv::core::lifecycle::CoreRegistrySnapshot& snapshot,
        int persist_attempt)>;

void set_core_registry_persist_candidate_hook(
    CoreRegistryPersistCandidateHook hook);
} // namespace exv::core::testing

static int g_failures = 0;

static bool expect(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "EXPECT FAILED: " << message << std::endl;
        g_failures++;
        return false;
    }
    return true;
}

// ---------------------------------------------------------------------------
// Thread-safe blocking input stream buffer (simulates stdin)
// ---------------------------------------------------------------------------

class BlockingInputBuf : public std::streambuf {
    std::mutex mtx_;
    std::condition_variable cv_;
    std::string pending_;
    bool closed_ = false;

    static constexpr size_t BUF_SZ = 4096;
    char get_area_[BUF_SZ];

protected:
    int_type underflow() override {
        std::unique_lock<std::mutex> lk(mtx_);
        cv_.wait(lk, [this] { return !pending_.empty() || closed_; });

        if (pending_.empty() && closed_) {
            return traits_type::eof();
        }

        size_t n = std::min(pending_.size(), BUF_SZ);
        std::memcpy(get_area_, pending_.data(), n);
        pending_.erase(0, n);

        setg(get_area_, get_area_, get_area_ + n);
        return traits_type::to_int_type(*gptr());
    }

public:
    void feed(const std::string& data) {
        {
            std::lock_guard<std::mutex> lk(mtx_);
            pending_ += data;
        }
        cv_.notify_all();
    }

    void close_input() {
        {
            std::lock_guard<std::mutex> lk(mtx_);
            closed_ = true;
        }
        cv_.notify_all();
    }
};

// ---------------------------------------------------------------------------
// Thread-safe output capture buffer (captures stdout)
// ---------------------------------------------------------------------------

class CaptureOutputBuf : public std::streambuf {
    mutable std::mutex mtx_;
    std::condition_variable cv_;
    std::string captured_;

protected:
    int_type overflow(int_type ch) override {
        if (ch != traits_type::eof()) {
            std::lock_guard<std::mutex> lk(mtx_);
            captured_ += static_cast<char>(ch);
            cv_.notify_all();
        }
        return ch;
    }

    std::streamsize xsputn(const char* s, std::streamsize n) override {
        std::lock_guard<std::mutex> lk(mtx_);
        captured_.append(s, static_cast<size_t>(n));
        cv_.notify_all();
        return n;
    }

    int sync() override {
        return 0; // always report success to avoid badbit from tie flushes
    }

public:
    // Non-destructive read (for polling loops)
    std::string get_all() const {
        std::lock_guard<std::mutex> lk(mtx_);
        return captured_;
    }

    // Destructive read (moves data out)
    std::string read_all() {
        std::lock_guard<std::mutex> lk(mtx_);
        std::string r = std::move(captured_);
        captured_.clear();
        return r;
    }

    bool wait_for_data(std::chrono::milliseconds timeout) {
        std::unique_lock<std::mutex> lk(mtx_);
        return cv_.wait_for(lk, timeout, [this] { return !captured_.empty(); });
    }
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

static std::string make_temp_dir() {
#ifdef _WIN32
    char tmp[MAX_PATH];
    GetTempPathA(MAX_PATH, tmp);
    auto ts = static_cast<long long>(
        std::chrono::steady_clock::now().time_since_epoch().count());
    char dir[MAX_PATH];
    sprintf_s(dir, "%s\\exv_lc_%lld", tmp, ts);
    _mkdir(dir);
    return std::string(dir);
#else
    std::string tmpl = "/tmp/exv_lc_XXXXXX";
    std::vector<char> buf(tmpl.begin(), tmpl.end());
    buf.push_back('\0');
    if (char* r = mkdtemp(buf.data())) return std::string(r);
    return "/tmp/exv_lc_fallback";
#endif
}

static void write_valid_native_config(const std::string& config_dir) {
    json cfg = {
        {"server", "https://vpn.example.invalid"},
        {"username", "student@example.invalid"},
        {"password", ""},
        {"remember_password", false},
        {"vpn_engine", "native"},
        {"windows_tunnel_driver", "tap"},
        {"windows_tap_interface", "ECNU VPN TAP"},
        {"auto_reconnect", false},
        {"extra_args", json::array()}
    };
    std::ofstream out(std::filesystem::path(config_dir) / "config.json",
                      std::ios::out | std::ios::trunc);
    out << cfg.dump(2);
}

static std::vector<json> parse_json_lines(const std::string& output) {
    std::vector<json> results;
    std::istringstream iss(output);
    std::string line;
    while (std::getline(iss, line)) {
        if (line.empty() || line.find_first_not_of(" \t\r\n") == std::string::npos)
            continue;
        try {
            results.push_back(json::parse(line));
        } catch (...) {
            // skip non-JSON lines (logger output, etc.)
        }
    }
    return results;
}

static bool wait_for_response_count(CaptureOutputBuf& buf, size_t min_count,
                                     std::chrono::milliseconds timeout) {
    auto deadline = std::chrono::steady_clock::now() + timeout;
    while (std::chrono::steady_clock::now() < deadline) {
        auto parsed = parse_json_lines(buf.get_all());
        size_t responses = 0;
        for (const auto& item : parsed) {
            if (item.is_object() && (item.contains("id") ||
                                     item.contains("request_id"))) {
                ++responses;
            }
        }
        if (responses >= min_count) return true;
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
    return false;
}

static bool wait_for_registry_state(
    const std::string& path,
    exv::core::lifecycle::CoreRegistryReadState state,
    std::chrono::milliseconds timeout) {
    auto deadline = std::chrono::steady_clock::now() + timeout;
    while (std::chrono::steady_clock::now() < deadline) {
        auto loaded = exv::core::lifecycle::read_core_registry(path);
        if (loaded.state == state) {
            return true;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
    return false;
}

static bool wait_for_atomic_at_least(const std::atomic<int>& value,
                                     int minimum,
                                     std::chrono::milliseconds timeout) {
    auto deadline = std::chrono::steady_clock::now() + timeout;
    while (std::chrono::steady_clock::now() < deadline) {
        if (value.load() >= minimum) {
            return true;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
    return false;
}

static json find_by_id(const std::vector<json>& responses, int id) {
    for (auto& r : responses) {
        if (r.value("id", -1) == id) return r;
    }
    return json();
}

static json find_by_request_id(const std::vector<json>& responses,
                               const std::string& request_id) {
    for (auto& r : responses) {
        if (r.value("request_id", std::string()) == request_id) return r;
    }
    return json();
}

static bool wait_for_response_id(CaptureOutputBuf& buf, int id,
                                 std::chrono::milliseconds timeout) {
    auto deadline = std::chrono::steady_clock::now() + timeout;
    while (std::chrono::steady_clock::now() < deadline) {
        auto parsed = parse_json_lines(buf.get_all());
        if (!find_by_id(parsed, id).is_null()) {
            return true;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(25));
    }
    return false;
}

static bool wait_for_request_id(CaptureOutputBuf& buf,
                                const std::string& request_id,
                                std::chrono::milliseconds timeout) {
    auto deadline = std::chrono::steady_clock::now() + timeout;
    while (std::chrono::steady_clock::now() < deadline) {
        auto parsed = parse_json_lines(buf.get_all());
        if (!find_by_request_id(parsed, request_id).is_null()) {
            return true;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(25));
    }
    return false;
}

// ---------------------------------------------------------------------------
// Scoped rdbuf redirector — restores original rdbuf on destruction
// ---------------------------------------------------------------------------

class ScopedRdbuf {
    std::ios& stream_;
    std::streambuf* orig_;
public:
    ScopedRdbuf(std::ios& s, std::streambuf* new_buf)
        : stream_(s), orig_(s.rdbuf()) {
        stream_.rdbuf(new_buf);
    }
    ~ScopedRdbuf() { stream_.rdbuf(orig_); }
    ScopedRdbuf(const ScopedRdbuf&) = delete;
    ScopedRdbuf& operator=(const ScopedRdbuf&) = delete;
};

// ---------------------------------------------------------------------------
// Run core_process_main in a thread and return {thread, exit_code_ref}
// ---------------------------------------------------------------------------

struct CoreRunner {
    std::thread thread;
    std::atomic<int> rc{-1};

    void start(const std::string& config_dir, const std::string& home_dir) {
        thread = std::thread([this, config_dir, home_dir] {
            rc = exv::core::core_process_main(config_dir, home_dir, true);
        });
    }

    void join() {
        if (thread.joinable()) thread.join();
    }
};

// ===========================================================================
// Tests — order matters: E2.1d (SIGTERM) MUST be last because it sets the
//         process-global g_stop_requested flag which is never reset.
// ===========================================================================

int main() {
    std::string config_dir = make_temp_dir();
    std::string home_dir   = make_temp_dir();
    std::cerr << "config_dir=" << config_dir << "  home_dir=" << home_dir << "\n";

    // =======================================================================
    // E2.0-regression — app_api vpn.connect returns actionable missing config
    // =======================================================================
    {
        std::cerr << "[E2.0-regression] direct app_api vpn.connect missing config\n";
        json payload = {
            {"home", home_dir},
            {"config_dir", config_dir},
            {"password", ""}
        };
        json result = ecnuvpn::app_api::handle_action("vpn.connect", payload);

        expect(result.is_object(),
               "E2.0-regression: app_api vpn.connect returns object");
        expect(result.value("ok", true) == false,
               "E2.0-regression: app_api vpn.connect fails with missing config");
        std::string error = result.value("error", std::string());
        std::string code = result.value("code", std::string());
        expect(!error.empty(),
               "E2.0-regression: app_api vpn.connect has actionable error message");
        expect(error.find("server") != std::string::npos ||
                   error.find("username") != std::string::npos ||
                   error.find("password") != std::string::npos ||
                   error.find("config") != std::string::npos ||
                   error.find("backend") != std::string::npos ||
                   code.find("config") != std::string::npos ||
                   code.find("backend") != std::string::npos,
               "E2.0-regression: app_api error names missing config/backend prerequisite");
        expect(result.value("status", std::string()) != "connecting",
               "E2.0-regression: app_api must not return blind connecting status");
    }

    // =======================================================================
    // E2.1a — Start and respond to status.get
    // =======================================================================
    {
        std::cerr << "[E2.1a] start + status.get\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        in_buf.feed(R"({"id":1,"action":"status.get","payload":{}})" "\n");
        expect(wait_for_response_count(out_buf, 1, std::chrono::seconds(3)),
               "E2.1a: should receive response");

        std::raise(SIGTERM);
        in_buf.close_input();
        cr.join();

        auto resp = find_by_id(parse_json_lines(out_buf.read_all()), 1);
        expect(!resp.is_null(),          "E2.1a: response exists");
        expect(resp.value("ok", false),  "E2.1a: ok=true");
        if (resp.contains("data")) {
            expect(resp["data"].contains("connected"),
                   "E2.1a: desktop status payload has connected field");
        }
        expect(cr.rc == 0, "E2.1a: exit code 0");
    }

    // =======================================================================
    // E2.1a-native — core native envelope routes through AppRpcDispatcher
    // =======================================================================
    {
        std::cerr << "[E2.1a-native] native core envelope dispatch\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);
        const auto registry_path =
            exv::core::lifecycle::core_registry_path(config_dir);

        in_buf.feed(R"({"request_id":"native-1","action":"vpn.status","payload_json":"{}"})" "\n");
        in_buf.feed(R"({"request_id":"native-hello","action":"core.hello","payload_json":"{\"contract_version\":\"2026-06-16.cli-core-ui-contract.v1\"}"})" "\n");
        in_buf.feed(R"({"request_id":"native-2","action":"missing.native","payload_json":"{}"})" "\n");

        expect(wait_for_response_count(out_buf, 3, std::chrono::seconds(5)),
               "E2.1a-native: should receive native responses");
        expect(wait_for_registry_state(
                   registry_path,
                   exv::core::lifecycle::CoreRegistryReadState::present,
                   std::chrono::seconds(5)),
               "E2.1a-native: registry should be written while core is running");

        auto registry = exv::core::lifecycle::read_core_registry(registry_path);
        expect(registry.state == exv::core::lifecycle::CoreRegistryReadState::present,
               "E2.1a-native: registry should exist before shutdown cleanup");
        if (registry.snapshot.has_value()) {
            expect(registry.snapshot->ipc_protocol_version == "ipc-v1",
                   "E2.1a-native: registry includes versioned ipc protocol");
            expect(registry.snapshot->ipc_path == exv::core::core_pipe_path(),
                   "E2.1a-native: registry uses current core pipe path");
            expect(registry.snapshot->last_known_tunnel_phase == "idle",
                   "E2.1a-native: registry starts with idle tunnel phase");
        }

        std::raise(SIGTERM);
        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());
        auto status_resp = find_by_request_id(all, "native-1");
        auto hello_resp = find_by_request_id(all, "native-hello");
        auto unknown_resp = find_by_request_id(all, "native-2");

        expect(!status_resp.is_null(),
               "E2.1a-native: vpn.status response exists");
        if (!status_resp.is_null()) {
            expect(status_resp.value("success", false),
                   "E2.1a-native: vpn.status success=true");
        }
        if (!status_resp.is_null() && status_resp.contains("payload_json")) {
            auto payload = json::parse(status_resp["payload_json"].get<std::string>());
            expect(payload.value("phase", std::string()) == "idle",
                   "E2.1a-native: vpn.status payload phase is idle");
        }

        expect(!hello_resp.is_null(),
               "E2.1a-native: core.hello response exists");
        if (!hello_resp.is_null()) {
            expect(hello_resp.value("success", false),
                   "E2.1a-native: core.hello success=true");
        }
        if (!hello_resp.is_null() && hello_resp.contains("payload_json")) {
            auto payload = json::parse(hello_resp["payload_json"].get<std::string>());
            expect(payload.value("ipc_protocol_version", std::string()) == "ipc-v1",
                   "E2.1a-native: core.hello payload includes ipc protocol version");
            expect(payload.value("contract_version", std::string()) ==
                       std::string(exv::contracts::generated::CONTRACT_VERSION),
                   "E2.1a-native: core.hello payload includes contract version");
            expect(payload.value("app_version", std::string()) == ECNUVPN_VERSION,
                   "E2.1a-native: core.hello payload includes app version");
            expect(payload.contains("core_instance_id") &&
                       payload["core_instance_id"].is_string() &&
                       !payload["core_instance_id"].get<std::string>().empty(),
                   "E2.1a-native: core.hello payload includes non-empty instance id");
        }

        expect(!unknown_resp.is_null(),
               "E2.1a-native: unknown native response exists");
        if (!unknown_resp.is_null()) {
            expect(unknown_resp.value("success", true) == false,
                   "E2.1a-native: unknown native action fails");
            expect(unknown_resp.value("error_code", std::string()) == "unknown_action",
                   "E2.1a-native: unknown native action keeps native error code");
        }
        expect(exv::core::lifecycle::read_core_registry(registry_path).state ==
                   exv::core::lifecycle::CoreRegistryReadState::missing,
               "E2.1a-native: registry should be removed on shutdown");
        expect(cr.rc == 0, "E2.1a-native: exit code 0");
    }

    // =======================================================================
    // E2.1a-registry-drift — failed post-start writes must not poison shutdown delete
    // =======================================================================
    {
        std::cerr << "[E2.1a-registry-drift] failed heartbeat write keeps persisted delete token\n";
        const auto registry_path =
            exv::core::lifecycle::core_registry_path(config_dir);

        std::atomic<int> persist_attempts{0};
        std::atomic<bool> fail_future_persist{false};
        exv::core::testing::set_core_registry_persist_candidate_hook(
            [&](exv::core::lifecycle::CoreRegistrySnapshot& snapshot,
                int persist_attempt) {
                persist_attempts.store(persist_attempt);
                if (fail_future_persist.load()) {
                    snapshot.helper_core_lease_id = "drifted-after-failed-write";
                }
            });
        exv::core::lifecycle::testing::set_write_core_registry_hook(
            [](const exv::core::lifecycle::CoreRegistrySnapshot& snapshot,
               const std::string&) -> std::optional<bool> {
                if (snapshot.helper_core_lease_id ==
                    "drifted-after-failed-write") {
                    return false;
                }
                return std::nullopt;
            });

        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin, &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        expect(wait_for_registry_state(
                   registry_path,
                   exv::core::lifecycle::CoreRegistryReadState::present,
                   std::chrono::seconds(5)),
               "E2.1a-registry-drift: initial registry write should succeed");
        const int attempts_before_failure = persist_attempts.load();
        fail_future_persist.store(true);
        expect(wait_for_atomic_at_least(persist_attempts, 2,
                                        std::chrono::seconds(5)),
               "E2.1a-registry-drift: post-start registry write should be attempted");
        expect(wait_for_atomic_at_least(persist_attempts,
                                        attempts_before_failure + 1,
                                        std::chrono::seconds(5)),
               "E2.1a-registry-drift: post-start failed registry write should be observed");

        in_buf.close_input();
        cr.join();

        exv::core::testing::set_core_registry_persist_candidate_hook(nullptr);
        exv::core::lifecycle::testing::set_write_core_registry_hook(nullptr);

        expect(cr.rc == 0, "E2.1a-registry-drift: exit code 0");
        expect(exv::core::lifecycle::read_core_registry(registry_path).state ==
                   exv::core::lifecycle::CoreRegistryReadState::missing,
               "E2.1a-registry-drift: shutdown should delete the last persisted registry");
    }

    // =======================================================================
    // E2.1a-regression — Desktop actions use real app_api handlers
    // =======================================================================
    {
        std::cerr << "[E2.1a-regression] logs.list + vpn.connect desktop bridge\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        in_buf.feed(R"({"id":30,"action":"logs.list","payload":{}})" "\n");
        in_buf.feed(R"({"id":31,"action":"vpn.connect","payload":{}})" "\n");

        expect(wait_for_response_count(out_buf, 2, std::chrono::seconds(5)),
               "E2.1a-regression: should get logs.list and vpn.connect responses");

        std::raise(SIGTERM);
        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());
        auto logs_resp = find_by_id(all, 30);
        auto connect_resp = find_by_id(all, 31);

        expect(!logs_resp.is_null(), "E2.1a-regression: logs.list response exists");
        expect(logs_resp.value("ok", false), "E2.1a-regression: logs.list ok=true");
        expect(logs_resp.contains("data") && logs_resp["data"].is_array(),
               "E2.1a-regression: logs.list data is an array");

        expect(!connect_resp.is_null(), "E2.1a-regression: vpn.connect response exists");
        expect(connect_resp.value("ok", true) == false,
               "E2.1a-regression: vpn.connect fails with empty config");
        std::string message = connect_resp.value("message", std::string());
        std::string code = connect_resp.value("code", std::string());
        expect(!message.empty(), "E2.1a-regression: vpn.connect error has message");
        expect(message.find("server") != std::string::npos ||
                   message.find("username") != std::string::npos ||
                   message.find("password") != std::string::npos ||
                   message.find("config") != std::string::npos ||
                   message.find("backend") != std::string::npos ||
                   code.find("config") != std::string::npos ||
                   code.find("backend") != std::string::npos,
               "E2.1a-regression: vpn.connect error is actionable config/backend failure");
        expect(!(connect_resp.contains("data") &&
                 connect_resp["data"].is_object() &&
                 connect_resp["data"].value("status", "") == "connecting"),
               "E2.1a-regression: vpn.connect must not return blind connecting success");
        expect(cr.rc == 0, "E2.1a-regression: exit code 0");
    }

    // =======================================================================
    // E2.1a-native-regression — Native envelope uses desktop VPN pipeline
    // =======================================================================
    {
        std::cerr << "[E2.1a-native-regression] native vpn.connect missing config\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        in_buf.feed(R"({"request_id":"native-connect-missing","action":"vpn.connect","payload_json":"{}"})" "\n");
        expect(wait_for_request_id(out_buf, "native-connect-missing",
                                   std::chrono::seconds(5)),
               "E2.1a-native-regression: native vpn.connect should respond");

        std::raise(SIGTERM);
        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());
        auto connect_resp = find_by_request_id(all, "native-connect-missing");

        expect(!connect_resp.is_null(),
               "E2.1a-native-regression: native vpn.connect response exists");
        expect(connect_resp.value("success", true) == false,
               "E2.1a-native-regression: native vpn.connect fails with missing config");
        std::string message =
            connect_resp.value("error_message", std::string());
        std::string code = connect_resp.value("error_code", std::string());
        expect(!message.empty(),
               "E2.1a-native-regression: native vpn.connect error has message");
        expect(message.find("server") != std::string::npos ||
                   message.find("username") != std::string::npos ||
                   message.find("password") != std::string::npos ||
                   message.find("config") != std::string::npos ||
                   message.find("backend") != std::string::npos ||
                   code.find("config") != std::string::npos ||
                   code.find("backend") != std::string::npos,
               "E2.1a-native-regression: native vpn.connect error names config/backend prerequisite");
        if (connect_resp.contains("payload_json") &&
            connect_resp["payload_json"].is_string()) {
            expect(connect_resp["payload_json"].get<std::string>().find("connecting") ==
                       std::string::npos,
                   "E2.1a-native-regression: native vpn.connect must not return blind connecting payload");
        }
        expect(cr.rc == 0, "E2.1a-native-regression: exit code 0");
    }

    // =======================================================================
    // E2.1a-pipe — stdin mode also services pipe clients through same bridge
    // =======================================================================
    {
        std::cerr << "[E2.1a-pipe] pipe request while stdin is open\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        exv::cli::PipeClient client;
        expect(exv::core::core_pipe_path().find("ipc-v1") != std::string::npos,
               "E2.1a-pipe: core_pipe_path should use versioned ipc naming");
        bool connected = false;
        for (int i = 0; i < 20 && !connected; ++i) {
            connected = client.connect(exv::core::core_pipe_path());
            if (!connected) std::this_thread::sleep_for(std::chrono::milliseconds(50));
        }
        expect(connected, "E2.1a-pipe: pipe client connects to stdin-mode core");

        std::string raw = client.send_request(R"({"id":40,"action":"logs.list","payload":{}})");
        client.disconnect();

#ifdef _WIN32
        HANDLE delayed = INVALID_HANDLE_VALUE;
        for (int i = 0; i < 20 && delayed == INVALID_HANDLE_VALUE; ++i) {
            delayed = CreateFileA(
                exv::core::core_pipe_path().c_str(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                nullptr,
                OPEN_EXISTING,
                0,
                nullptr);
            if (delayed == INVALID_HANDLE_VALUE) {
                std::this_thread::sleep_for(std::chrono::milliseconds(50));
            }
        }
        expect(delayed != INVALID_HANDLE_VALUE,
               "E2.1a-pipe: delayed-write pipe client connects");
        std::string delayed_raw;
        if (delayed != INVALID_HANDLE_VALUE) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
            const std::string request = R"({"id":41,"action":"logs.list","payload":{}})" "\n";
            DWORD written = 0;
            BOOL wrote = WriteFile(delayed, request.c_str(),
                                   static_cast<DWORD>(request.size()),
                                   &written, nullptr);
            expect(wrote && written == request.size(),
                   "E2.1a-pipe: delayed-write pipe request writes after connect");
            char buffer[8192] = {};
            DWORD read = 0;
            BOOL read_ok = ReadFile(delayed, buffer, sizeof(buffer) - 1, &read, nullptr);
            expect(read_ok && read > 0,
                   "E2.1a-pipe: delayed-write pipe response is readable");
            if (read_ok && read > 0) delayed_raw.assign(buffer, read);
            CloseHandle(delayed);
        }
#endif

        std::raise(SIGTERM);
        in_buf.close_input();
        cr.join();

        json resp;
        try { resp = json::parse(raw); } catch (...) {}
        expect(!resp.is_null(), "E2.1a-pipe: pipe response parses as JSON");
        expect(resp.value("id", -1) == 40, "E2.1a-pipe: pipe response id matches");
        expect(resp.value("ok", false), "E2.1a-pipe: logs.list over pipe ok=true");
        expect(resp.contains("data") && resp["data"].is_array(),
               "E2.1a-pipe: pipe logs.list data is array");
#ifdef _WIN32
        json delayed_resp;
        try { delayed_resp = json::parse(delayed_raw); } catch (...) {}
        expect(!delayed_resp.is_null(), "E2.1a-pipe: delayed-write response parses as JSON");
        if (!delayed_resp.is_null()) {
            expect(delayed_resp.value("id", -1) == 41,
                   "E2.1a-pipe: delayed-write response id matches");
            expect(delayed_resp.value("ok", false),
                   "E2.1a-pipe: delayed-write logs.list over pipe ok=true");
        }
#endif
        expect(cr.rc == 0, "E2.1a-pipe: exit code 0");
    }

    // =======================================================================
    // E2.1b — Multiple sequential requests
    // =======================================================================
    {
        std::cerr << "[E2.1b] multiple sequential requests\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        in_buf.feed(R"({"id":1,"action":"status.get","payload":{}})"          "\n");
        in_buf.feed(R"({"id":2,"action":"runtime.status","payload":{}})"       "\n");
        in_buf.feed(R"({"id":3,"action":"nonexistent.action","payload":{}})"   "\n");

        expect(wait_for_response_count(out_buf, 3, std::chrono::seconds(5)),
               "E2.1b: should get 3 responses");

        std::raise(SIGTERM);
        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());
        auto r1 = find_by_id(all, 1);
        auto r2 = find_by_id(all, 2);
        auto r3 = find_by_id(all, 3);

        expect(!r1.is_null() && r1.value("ok", false),
               "E2.1b: status.get ok");
        expect(!r2.is_null() && r2.value("ok", false),
               "E2.1b: runtime.status ok");
        expect(!r3.is_null() && !r3.value("ok", true),
               "E2.1b: unknown action fails");
        expect(cr.rc == 0, "E2.1b: exit code 0");
    }

    // =======================================================================
    // E2.1c — Graceful shutdown on stdin EOF
    // =======================================================================
    {
        std::cerr << "[E2.1c] graceful shutdown on EOF\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        // Let process start (it blocks on getline), then close stdin immediately.
        // EOF alone must shut stdin-mode core down cleanly; do not signal it.
        std::this_thread::sleep_for(std::chrono::milliseconds(200));
        in_buf.close_input();
        cr.join();

        expect(cr.rc == 0, "E2.1c: exit 0 on EOF");
    }

    // =======================================================================
    // E2.2a — Malformed JSON input (crash recovery)
    // =======================================================================
    {
        std::cerr << "[E2.2a] malformed JSON\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        // Feed malformed + valid input upfront to avoid timing issues
        in_buf.feed("this is definitely not json\n");
        in_buf.feed(R"({"id":10,"action":"status.get","payload":{}})" "\n");

        // Wait for both responses (parse_error + valid)
        expect(wait_for_response_count(out_buf, 2, std::chrono::seconds(5)),
               "E2.2a: parse_error + valid response");

        std::raise(SIGTERM);
        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());

        // Malformed JSON response should be a parse_error (id=0)
        auto bad_resp = find_by_id(all, 0);
        if (!bad_resp.is_null()) {
            expect(bad_resp.value("ok", true) == false,
                   "E2.2a: bad input returns ok=false");
            expect(bad_resp.value("code", "") == "parse_error",
                   "E2.2a: error code is parse_error");
        }

        // Second response should be valid (id=10)
        auto good_resp = find_by_id(all, 10);
        expect(!good_resp.is_null() && good_resp.value("ok", false),
               "E2.2a: valid request succeeds after bad input");

        expect(cr.rc == 0, "E2.2a: exit 0 (process survived)");
    }

    // =======================================================================
    // E2.2b — Missing action field
    // =======================================================================
    {
        std::cerr << "[E2.2b] missing action field\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        // Feed all input upfront
        in_buf.feed(R"({"id":1,"payload":{}})" "\n");
        in_buf.feed(R"({"id":2,"action":"status.get","payload":{}})" "\n");

        // Wait for both responses (missing_action + valid)
        expect(wait_for_response_count(out_buf, 2, std::chrono::seconds(5)),
               "E2.2b: missing_action + valid response");

        std::raise(SIGTERM);
        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());

        // Missing action response should be a missing_action error.
        auto missing_resp = find_by_id(all, 1);
        if (!missing_resp.is_null()) {
            expect(missing_resp.value("ok", true) == false,
                   "E2.2b: missing_action returns ok=false");
            expect(missing_resp.value("code", "") == "missing_action",
                   "E2.2b: code=missing_action");
        }

        // Second response: valid status.get
        auto valid_resp = find_by_id(all, 2);
        expect(!valid_resp.is_null() && valid_resp.value("ok", false),
               "E2.2b: valid request succeeds after missing action");

        expect(cr.rc == 0, "E2.2b: exit 0");
    }

    // =======================================================================
    // E2.2c — Mixed bad + good input (crash recovery sequence)
    // =======================================================================
    {
        std::cerr << "[E2.2c] mixed bad+good input burst\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        // Burst: malformed JSON, valid requests
        in_buf.feed("{bad json!!\n");
        in_buf.feed(R"({"id":20,"action":"status.get","payload":{}})" "\n");
        in_buf.feed(R"({"id":21,"action":"logs.list","payload":{}})" "\n");

        expect(wait_for_response_count(out_buf, 3, std::chrono::seconds(5)),
               "E2.2c: got responses for parse_error + 2 valid requests");

        std::raise(SIGTERM);
        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());
        auto r20 = find_by_id(all, 20);
        auto r21 = find_by_id(all, 21);

        expect(!r20.is_null() && r20.value("ok", false),
               "E2.2c: status.get ok after burst");
        expect(!r21.is_null() && r21.value("ok", false) &&
               r21.contains("data") && r21["data"].is_array(),
               "E2.2c: logs.list ok after burst");
        expect(cr.rc == 0, "E2.2c: exit 0");
    }

    // =======================================================================
    // E2.2d — Initial registry write failure aborts startup
    // =======================================================================
    {
        std::cerr << "[E2.2d] initial registry write failure aborts startup\n";
        namespace fs = std::filesystem;
        std::error_code ec;
        fs::remove_all(config_dir, ec);
        {
            std::ofstream blocker(config_dir, std::ios::out | std::ios::trunc);
            blocker << "not-a-directory";
        }

        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        std::this_thread::sleep_for(std::chrono::milliseconds(200));

        exv::cli::PipeClient client;
        bool connected = false;
        for (int i = 0; i < 10 && !connected; ++i) {
            connected = client.connect(exv::core::core_pipe_path());
            if (!connected) {
                std::this_thread::sleep_for(std::chrono::milliseconds(50));
            }
        }
        expect(!connected,
               "E2.2d: core must not keep accepting pipe clients after initial registry write failure");
        if (connected) {
            client.disconnect();
        }

        in_buf.close_input();
        cr.join();

        expect(cr.rc != 0,
               "E2.2d: initial registry write failure must return non-zero");

        fs::remove(config_dir, ec);
        fs::create_directories(config_dir, ec);
    }

    // =======================================================================
    // E2.3-lanes — long vpn.connect does not block read-only lanes
    // =======================================================================
    {
        std::cerr << "[E2.3-lanes] logs/status/config while vpn.connect is blocked\n";
        write_valid_native_config(config_dir);

        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        std::promise<void> connect_entered_promise;
        std::shared_future<void> connect_entered =
            connect_entered_promise.get_future().share();
        std::promise<void> release_connect_promise;
        std::shared_future<void> release_connect =
            release_connect_promise.get_future().share();
        std::atomic<bool> connect_entered_once{false};

        ecnuvpn::app_api::testing::set_desktop_vpn_connect_entered_hook(
            [&] {
                if (!connect_entered_once.exchange(true)) {
                    connect_entered_promise.set_value();
                }
                release_connect.wait();
            });

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        in_buf.feed(R"({"id":50,"action":"vpn.connect","payload":{"password":"x"}})" "\n");
        expect(connect_entered.wait_for(std::chrono::seconds(3)) ==
                   std::future_status::ready,
               "E2.3-lanes: vpn.connect background job should enter blocking hook");
        expect(wait_for_response_id(out_buf, 50, std::chrono::milliseconds(500)),
               "E2.3-lanes: vpn.connect should return accepted while background job remains blocked");

        in_buf.feed(R"({"id":51,"action":"logs.list","payload":{}})" "\n");
        expect(wait_for_response_id(out_buf, 51, std::chrono::milliseconds(500)),
               "E2.3-lanes: logs.list should respond while vpn.connect remains blocked");
        in_buf.feed(R"({"id":52,"action":"status.get","payload":{}})" "\n");
        expect(wait_for_response_id(out_buf, 52, std::chrono::milliseconds(500)),
               "E2.3-lanes: status.get should respond while vpn.connect remains blocked");
        in_buf.feed(R"({"id":53,"action":"config.getSettings","payload":{}})" "\n");
        expect(wait_for_response_id(out_buf, 53, std::chrono::milliseconds(500)),
               "E2.3-lanes: config.getSettings should respond while vpn.connect remains blocked");

        release_connect_promise.set_value();
        expect(wait_for_response_count(out_buf, 4, std::chrono::seconds(5)),
               "E2.3-lanes: accepted vpn.connect and read-only responses should all exist");

        ecnuvpn::app_api::testing::set_desktop_vpn_connect_entered_hook(nullptr);
        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());
        auto logs_resp = find_by_id(all, 51);
        auto status_resp = find_by_id(all, 52);
        auto settings_resp = find_by_id(all, 53);
        auto connect_resp = find_by_id(all, 50);
        expect(!logs_resp.is_null() && logs_resp.value("ok", false),
               "E2.3-lanes: logs.list response should be successful");
        expect(!status_resp.is_null() && status_resp.value("ok", false),
               "E2.3-lanes: status.get response should be successful");
        expect(!settings_resp.is_null() && settings_resp.value("ok", false),
               "E2.3-lanes: config.getSettings response should be successful");
        expect(!connect_resp.is_null(),
               "E2.3-lanes: vpn.connect response should eventually exist");
        expect(connect_resp.value("ok", false),
               "E2.3-lanes: vpn.connect accepted response should be successful");
        if (!connect_resp.is_null() && connect_resp.contains("data")) {
            expect(connect_resp["data"].value("accepted", false),
                   "E2.3-lanes: vpn.connect data should mark accepted=true");
            expect(connect_resp["data"].value("phase", std::string()) == "connecting",
                   "E2.3-lanes: vpn.connect accepted phase should be connecting");
        }
        if (!status_resp.is_null() && status_resp.contains("data")) {
            expect(status_resp["data"].value("process_running", false),
                   "E2.3-lanes: status.get should report accepted connect job as running before controller init");
            expect(status_resp["data"].value("phase", std::string()) == "connecting",
                   "E2.3-lanes: status.get should expose connecting phase for accepted connect job");
        }
        expect(cr.rc == 0, "E2.3-lanes: exit code 0");
    }

    // =======================================================================
    // E2.1d — Graceful shutdown on SIGTERM
    // MUST BE LAST: sets process-global g_stop_requested, never reset.
    // =======================================================================
    {
        std::cerr << "[E2.1d] graceful shutdown on SIGTERM\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        // Feed a request and wait for response
        in_buf.feed(R"({"id":1,"action":"status.get","payload":{}})" "\n");
        wait_for_response_count(out_buf, 1, std::chrono::seconds(3));

        // Process is now blocking on getline. Deliver SIGTERM.
        std::raise(SIGTERM);

        // Unblock getline so the loop can check g_stop_requested
        in_buf.close_input();
        cr.join();

        expect(cr.rc == 0, "E2.1d: exit 0 on SIGTERM");
    }

    // =======================================================================
    // Cleanup
    // =======================================================================
#ifdef _WIN32
    RemoveDirectoryA(config_dir.c_str());
    RemoveDirectoryA(home_dir.c_str());
#else
    rmdir(config_dir.c_str());
    rmdir(home_dir.c_str());
#endif

    if (g_failures == 0) {
        std::cout << "core_process_lifecycle_test: all assertions passed\n";
        return 0;
    } else {
        std::cerr << "core_process_lifecycle_test: " << g_failures
                  << " assertion(s) FAILED\n";
        return 1;
    }
}
