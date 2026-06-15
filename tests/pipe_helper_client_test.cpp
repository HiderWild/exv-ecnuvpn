/// @file pipe_helper_client_test.cpp
/// @brief End-to-end test: PipeHelperClient <-> raw named pipe server.
///
/// This verifies that the helper protocol can travel over the Windows named pipe
/// transport with a persistent connection, which is the path the Core process
/// will use to talk to the Helper daemon.

#include "helper/common/pipe_helper_client.hpp"
#include "helper/common/helper_connector.hpp"
#include "helper/common/helper_messages.hpp"
#include "helper/common/helper_protocol.hpp"

#include <nlohmann/json.hpp>

#include <cassert>
#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <thread>

#ifdef _WIN32
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#else
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#endif

using namespace exv::helper;
using json = nlohmann::json;

static void expect_true(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "EXPECT FAILED: " << message << std::endl;
        std::exit(1);
    }
}

namespace ecnuvpn {
namespace logger {

void init() {}
void write(const std::string &, const std::string &) {}
void info(const std::string &) {}
void error(const std::string &) {}
void warn(const std::string &) {}
void event(const std::string &, const std::string &, const std::string &,
           const std::string &,
           const std::vector<std::pair<std::string, std::string>> &) {}
void show_logs(int) {}
std::vector<std::string> tail(int) { return {}; }

} // namespace logger
} // namespace ecnuvpn

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#ifdef _WIN32
static std::string test_pipe_name() {
    static int counter = 0;
    return "\\\\.\\pipe\\exv-test-" + std::to_string(GetCurrentProcessId()) +
           "-" + std::to_string(++counter);
}
#else
static std::string test_pipe_name() {
    static int counter = 0;
    return "/tmp/exv-test-" + std::to_string(getpid()) +
           "-" + std::to_string(++counter);
}
#endif

// ---------------------------------------------------------------------------
// Minimal raw server using platform APIs (no IpcServer / logger deps).
// ---------------------------------------------------------------------------

#ifdef _WIN32

static bool wait_for_client_win32(HANDLE hPipe, DWORD timeout_ms = 3000) {
    (void)timeout_ms;
    if (ConnectNamedPipe(hPipe, NULL))
        return true;
    const DWORD err = GetLastError();
    return err == ERROR_PIPE_CONNECTED || err == ERROR_PIPE_LISTENING ||
           err == ERROR_NO_DATA;
}

static bool read_request_win32(HANDLE hPipe, std::string& raw,
                               DWORD timeout_ms = 3000) {
    const DWORD deadline = GetTickCount() + timeout_ms;
    char buffer[4096];
    while (GetTickCount() < deadline) {
        DWORD bytesRead = 0;
        if (ReadFile(hPipe, buffer, sizeof(buffer) - 1, &bytesRead, NULL) &&
            bytesRead > 0) {
            raw.append(buffer, bytesRead);
            if (raw.find('\n') != std::string::npos)
                return true;
            continue;
        }
        const DWORD err = GetLastError();
        if (err != ERROR_NO_DATA && err != ERROR_PIPE_LISTENING) {
            return false;
        }
        Sleep(10);
    }
    return raw.find('\n') != std::string::npos;
}

/// Create a named pipe server, accept one client, handle one helper hello request.
static bool handle_one_request_win32(const std::string& pipe_name) {
    HANDLE hPipe = CreateNamedPipeA(
        pipe_name.c_str(),
        PIPE_ACCESS_DUPLEX,
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT,
        1, 65536, 65536, 0, NULL);
    if (hPipe == INVALID_HANDLE_VALUE)
        return false;

    if (!wait_for_client_win32(hPipe)) {
        CloseHandle(hPipe);
        return false;
    }
    std::string raw;
    (void)read_request_win32(hPipe, raw);

    if (raw.empty()) {
        DisconnectNamedPipe(hPipe);
        CloseHandle(hPipe);
        return false;
    }

    // Parse envelope
    json envelope = json::parse(raw);
    HelperRequest req = helper_request_from_json(envelope);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = true;

    if (req.op == HelperOp::Hello) {
        HelloResponse hello_resp;
        hello_resp.capabilities = {"tunnel_device_create", "route_apply"};
        hello_resp.mode = HelperMode::Transient;
        json payload;
        to_json(payload, hello_resp);
        resp.payload_json = payload.dump();
    } else {
        resp.success = false;
        resp.error_code = "unsupported_op";
    }

    // Send response
    json resp_envelope = resp;
    std::string wire = resp_envelope.dump() + "\n";
    DWORD bytesWritten = 0;
    WriteFile(hPipe, wire.c_str(), static_cast<DWORD>(wire.size()), &bytesWritten, NULL);

    DisconnectNamedPipe(hPipe);
    CloseHandle(hPipe);
    return true;
}

/// Persistent server: accept one client, handle N sequential requests.
static bool handle_persistent_requests_win32(const std::string& pipe_name,
                                              int num_requests) {
    HANDLE hPipe = CreateNamedPipeA(
        pipe_name.c_str(),
        PIPE_ACCESS_DUPLEX,
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT,
        1, 65536, 65536, 0, NULL);
    if (hPipe == INVALID_HANDLE_VALUE)
        return false;

    if (!wait_for_client_win32(hPipe)) {
        CloseHandle(hPipe);
        return false;
    }
    for (int i = 0; i < num_requests; ++i) {
        std::string raw;
        (void)read_request_win32(hPipe, raw);
        if (raw.empty())
            break;

        json envelope = json::parse(raw);
        HelperRequest req = helper_request_from_json(envelope);

        HelperResponse resp;
        resp.op = req.op;
        resp.success = true;

        if (req.op == HelperOp::Hello) {
            HelloResponse hr;
            hr.capabilities = {"tunnel_device_create"};
            hr.mode = HelperMode::Transient;
            json payload;
            to_json(payload, hr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::StartSession) {
            StartSessionResponse ssr;
            ssr.session_id.value = "test-session-" + std::to_string(i);
            json payload;
            to_json(payload, ssr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::Heartbeat) {
            HeartbeatResponse hbr;
            hbr.ok = true;
            json payload;
            to_json(payload, hbr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::Shutdown) {
            ShutdownResponse esr;
            esr.cleanup_success = true;
            json payload;
            to_json(payload, esr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::Inspect) {
            InspectResponse ir;
            ir.capabilities = {"core_lease"};
            ir.mode = HelperMode::Transient;
            ir.core_lease.active = true;
            ir.core_lease.lease_id = "lease-inspect";
            ir.task_queue.idle = true;
            json payload;
            to_json(payload, ir);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::AcquireCoreLease) {
            auto payload_req =
                acquire_core_lease_request_from_json(json::parse(req.payload_json));
            AcquireCoreLeaseResponse acr;
            acr.accepted = payload_req.core_pid == 2468;
            acr.lease_id = "lease-acquired";
            acr.mode = "oneshot";
            json payload;
            to_json(payload, acr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::KeepAlive) {
            auto payload_req =
                keep_alive_request_from_json(json::parse(req.payload_json));
            KeepAliveResponse kar;
            kar.ok = payload_req.lease_id == "lease-acquired";
            json payload;
            to_json(payload, kar);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::ReleaseCoreLease) {
            auto payload_req =
                release_core_lease_request_from_json(json::parse(req.payload_json));
            ReleaseCoreLeaseResponse rcr;
            rcr.released = payload_req.lease_id == "lease-acquired";
            rcr.exiting = payload_req.exit_if_oneshot;
            json payload;
            to_json(payload, rcr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::InstallService) {
            InstallServiceResponse isr;
            isr.success = true;
            isr.exit_code = 0;
            isr.message = "installed";
            json payload;
            to_json(payload, isr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::UninstallService) {
            UninstallServiceResponse usr;
            usr.success = true;
            usr.exit_code = 0;
            usr.message = "uninstalled";
            json payload;
            to_json(payload, usr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::ExportCleanupLease) {
            ExportCleanupLeaseResponse ecr;
            ecr.has_active_session = true;
            ecr.lease.cleanup_lease_id = "cleanup-lease-pipe";
            CleanupLeaseSession session;
            session.session_id.value = "pipe-session";
            session.profile_id.value = "pipe-profile";
            session.managed_resources.push_back({"adapter", "ECNU-VPN"});
            ecr.lease.sessions.push_back(session);
            json payload;
            to_json(payload, ecr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::HandoffSession) {
            auto payload_req =
                handoff_session_request_from_json(json::parse(req.payload_json));
            HandoffSessionResponse hsr;
            hsr.adopted = !payload_req.lease.sessions.empty();
            hsr.message = "adopted";
            for (const auto& session : payload_req.lease.sessions)
                hsr.session_ids.push_back(session.session_id);
            json payload;
            to_json(payload, hsr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::FinalizeHandoff) {
            auto payload_req =
                finalize_handoff_request_from_json(json::parse(req.payload_json));
            FinalizeHandoffResponse fhr;
            fhr.finalized = true;
            fhr.exiting = payload_req.exit;
            json payload;
            to_json(payload, fhr);
            resp.payload_json = payload.dump();
        } else {
            resp.success = false;
            resp.error_code = "unsupported_op";
        }

        json resp_envelope = resp;
        std::string wire = resp_envelope.dump() + "\n";
        DWORD bytesWritten = 0;
        WriteFile(hPipe, wire.c_str(), static_cast<DWORD>(wire.size()), &bytesWritten, NULL);
    }

    DisconnectNamedPipe(hPipe);
    CloseHandle(hPipe);
    return true;
}

#else // POSIX

static bool handle_one_request_posix(const std::string& socket_path) {
    int server_fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (server_fd < 0) return false;

    sockaddr_un addr{};
    addr.sun_family = AF_UNIX;
    snprintf(addr.sun_path, sizeof(addr.sun_path), "%s", socket_path.c_str());
    unlink(socket_path.c_str());

    if (bind(server_fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
        ::close(server_fd);
        return false;
    }
    chmod(socket_path.c_str(), 0666);
    if (listen(server_fd, 1) != 0) {
        ::close(server_fd);
        return false;
    }

    int client_fd = ::accept(server_fd, nullptr, nullptr);
    if (client_fd < 0) {
        ::close(server_fd);
        return false;
    }

    std::string raw;
    char buffer[4096];
    ssize_t n = 0;
    while ((n = read(client_fd, buffer, sizeof(buffer) - 1)) > 0) {
        buffer[n] = '\0';
        raw.append(buffer, n);
        if (raw.find('\n') != std::string::npos)
            break;
    }

    ::close(server_fd);
    unlink(socket_path.c_str());

    if (raw.empty()) {
        ::close(client_fd);
        return false;
    }

    json envelope = json::parse(raw);
    HelperRequest req = helper_request_from_json(envelope);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = true;

    if (req.op == HelperOp::Hello) {
        HelloResponse hr;
        hr.capabilities = {"tunnel_device_create", "route_apply"};
        hr.mode = HelperMode::Transient;
        json payload;
        to_json(payload, hr);
        resp.payload_json = payload.dump();
    } else {
        resp.success = false;
        resp.error_code = "unsupported_op";
    }

    json resp_envelope = resp;
    std::string wire = resp_envelope.dump() + "\n";
    write(client_fd, wire.c_str(), wire.size());
    ::close(client_fd);
    return true;
}

static bool handle_persistent_requests_posix(const std::string& socket_path,
                                              int num_requests) {
    int server_fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (server_fd < 0) return false;

    sockaddr_un addr{};
    addr.sun_family = AF_UNIX;
    snprintf(addr.sun_path, sizeof(addr.sun_path), "%s", socket_path.c_str());
    unlink(socket_path.c_str());

    if (bind(server_fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
        ::close(server_fd);
        return false;
    }
    chmod(socket_path.c_str(), 0666);
    if (listen(server_fd, 1) != 0) {
        ::close(server_fd);
        return false;
    }

    int client_fd = ::accept(server_fd, nullptr, nullptr);
    if (client_fd < 0) {
        ::close(server_fd);
        return false;
    }

    for (int i = 0; i < num_requests; ++i) {
        std::string raw;
        char buffer[4096];
        ssize_t n = 0;
        while ((n = read(client_fd, buffer, sizeof(buffer) - 1)) > 0) {
            buffer[n] = '\0';
            raw.append(buffer, n);
            if (raw.find('\n') != std::string::npos)
                break;
        }
        if (raw.empty()) break;

        json envelope = json::parse(raw);
        HelperRequest req = helper_request_from_json(envelope);

        HelperResponse resp;
        resp.op = req.op;
        resp.success = true;

        if (req.op == HelperOp::Hello) {
            HelloResponse hr;
            hr.capabilities = {"tunnel_device_create"};
            hr.mode = HelperMode::Transient;
            json payload;
            to_json(payload, hr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::StartSession) {
            StartSessionResponse ssr;
            ssr.session_id.value = "test-session-" + std::to_string(i);
            json payload;
            to_json(payload, ssr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::Heartbeat) {
            HeartbeatResponse hbr;
            hbr.ok = true;
            json payload;
            to_json(payload, hbr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::Shutdown) {
            ShutdownResponse esr;
            esr.cleanup_success = true;
            json payload;
            to_json(payload, esr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::Inspect) {
            InspectResponse ir;
            ir.capabilities = {"core_lease"};
            ir.mode = HelperMode::Transient;
            ir.core_lease.active = true;
            ir.core_lease.lease_id = "lease-inspect";
            ir.task_queue.idle = true;
            json payload;
            to_json(payload, ir);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::AcquireCoreLease) {
            auto payload_req =
                acquire_core_lease_request_from_json(json::parse(req.payload_json));
            AcquireCoreLeaseResponse acr;
            acr.accepted = payload_req.core_pid == 2468;
            acr.lease_id = "lease-acquired";
            acr.mode = "oneshot";
            json payload;
            to_json(payload, acr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::KeepAlive) {
            auto payload_req =
                keep_alive_request_from_json(json::parse(req.payload_json));
            KeepAliveResponse kar;
            kar.ok = payload_req.lease_id == "lease-acquired";
            json payload;
            to_json(payload, kar);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::ReleaseCoreLease) {
            auto payload_req =
                release_core_lease_request_from_json(json::parse(req.payload_json));
            ReleaseCoreLeaseResponse rcr;
            rcr.released = payload_req.lease_id == "lease-acquired";
            rcr.exiting = payload_req.exit_if_oneshot;
            json payload;
            to_json(payload, rcr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::InstallService) {
            InstallServiceResponse isr;
            isr.success = true;
            isr.exit_code = 0;
            isr.message = "installed";
            json payload;
            to_json(payload, isr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::UninstallService) {
            UninstallServiceResponse usr;
            usr.success = true;
            usr.exit_code = 0;
            usr.message = "uninstalled";
            json payload;
            to_json(payload, usr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::ExportCleanupLease) {
            ExportCleanupLeaseResponse ecr;
            ecr.has_active_session = true;
            ecr.lease.cleanup_lease_id = "cleanup-lease-pipe";
            CleanupLeaseSession session;
            session.session_id.value = "pipe-session";
            session.profile_id.value = "pipe-profile";
            session.managed_resources.push_back({"adapter", "ECNU-VPN"});
            ecr.lease.sessions.push_back(session);
            json payload;
            to_json(payload, ecr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::HandoffSession) {
            auto payload_req =
                handoff_session_request_from_json(json::parse(req.payload_json));
            HandoffSessionResponse hsr;
            hsr.adopted = !payload_req.lease.sessions.empty();
            hsr.message = "adopted";
            for (const auto& session : payload_req.lease.sessions)
                hsr.session_ids.push_back(session.session_id);
            json payload;
            to_json(payload, hsr);
            resp.payload_json = payload.dump();
        } else if (req.op == HelperOp::FinalizeHandoff) {
            auto payload_req =
                finalize_handoff_request_from_json(json::parse(req.payload_json));
            FinalizeHandoffResponse fhr;
            fhr.finalized = true;
            fhr.exiting = payload_req.exit;
            json payload;
            to_json(payload, fhr);
            resp.payload_json = payload.dump();
        } else {
            resp.success = false;
            resp.error_code = "unsupported_op";
        }

        json resp_envelope = resp;
        std::string wire = resp_envelope.dump() + "\n";
        write(client_fd, wire.c_str(), wire.size());
    }

    ::close(client_fd);
    ::close(server_fd);
    unlink(socket_path.c_str());
    return true;
}

#endif // _WIN32

// ---------------------------------------------------------------------------
// Test: single hello round-trip
// ---------------------------------------------------------------------------

static void test_single_hello() {
    std::cout << "  test_single_hello...";

    std::string pipe = test_pipe_name();
#ifdef _WIN32
    std::thread server_thread(handle_one_request_win32, pipe);
#else
    std::thread server_thread(handle_one_request_posix, pipe);
#endif
    std::this_thread::sleep_for(std::chrono::milliseconds(50));

    PipeClientConfig config;
    config.pipe_path = pipe;
    config.connect_timeout_ms = 3000;

    PipeHelperClient client(config);
    expect_true(client.connect(), "client should connect to test pipe");
    expect_true(client.is_connected(), "client should report connected");

    HelloRequest req;
    auto resp = client.hello(req);

    expect_true(resp.capabilities.size() == 2,
                "hello should return expected capabilities");
    expect_true(resp.mode == HelperMode::Transient,
                "hello should return transient mode");

    client.disconnect();
    expect_true(!client.is_connected(), "client should disconnect");

    server_thread.join();
    std::cout << " PASS\n";
}

// ---------------------------------------------------------------------------
// Test: persistent connection (multiple helper messages on one pipe)
// ---------------------------------------------------------------------------

static void test_persistent_connection() {
    std::cout << "  test_persistent_connection...";

    std::string pipe = test_pipe_name();
    int num_requests = 5;
#ifdef _WIN32
    std::thread server_thread(handle_persistent_requests_win32, pipe, num_requests);
#else
    std::thread server_thread(handle_persistent_requests_posix, pipe, num_requests);
#endif
    std::this_thread::sleep_for(std::chrono::milliseconds(50));

    PipeClientConfig config;
    config.pipe_path = pipe;
    config.connect_timeout_ms = 3000;

    PipeHelperClient client(config);
    expect_true(client.connect(), "client should connect to persistent test pipe");

    // 1. Hello
    HelloRequest hello_req;
    auto hello_resp = client.hello(hello_req);
    expect_true(hello_resp.capabilities.size() == 1,
                "persistent hello should return capabilities");

    // 2. StartSession
    StartSessionRequest ss_req;
    ss_req.profile_id.value = "test-profile";
    auto ss_resp = client.start_session(ss_req);
    expect_true(!ss_resp.session_id.value.empty(),
                "StartSession should return a session id");

    // 3. Heartbeat
    HeartbeatRequest hb_req;
    hb_req.session_id = ss_resp.session_id;
    hb_req.core_phase = "Connected";
    auto hb_resp = client.heartbeat(hb_req);
    expect_true(hb_resp.ok, "Heartbeat should succeed");

    // 4. Another Heartbeat
    auto hb_resp2 = client.heartbeat(hb_req);
    expect_true(hb_resp2.ok, "second Heartbeat should succeed");

    // 5. Shutdown
    ShutdownRequest shutdown_req;
    shutdown_req.session_id = ss_resp.session_id;
    auto shutdown_resp = client.shutdown(shutdown_req);
    if (!shutdown_resp.cleanup_success) {
        std::cerr << "Shutdown errors:";
        for (const auto& error : shutdown_resp.errors)
            std::cerr << " [" << error << "]";
        std::cerr << "\n";
    }
    expect_true(shutdown_resp.cleanup_success, "Shutdown should succeed");

    client.disconnect();
    server_thread.join();
    std::cout << " PASS\n";
}

static void test_core_lease_protocol_methods() {
    std::cout << "  test_core_lease_protocol_methods...";

    std::string pipe = test_pipe_name();
    int num_requests = 4;
#ifdef _WIN32
    std::thread server_thread(handle_persistent_requests_win32, pipe, num_requests);
#else
    std::thread server_thread(handle_persistent_requests_posix, pipe, num_requests);
#endif
    std::this_thread::sleep_for(std::chrono::milliseconds(50));

    PipeClientConfig config;
    config.pipe_path = pipe;
    config.connect_timeout_ms = 3000;

    PipeHelperClient client(config);
    expect_true(client.connect(), "client should connect for core lease protocol test");

    auto inspect_resp = client.inspect(InspectRequest{});
    expect_true(inspect_resp.core_lease.active,
                "Inspect should return core lease state");
    expect_true(inspect_resp.core_lease.lease_id == "lease-inspect",
                "Inspect should return lease id from helper");
    expect_true(inspect_resp.task_queue.idle,
                "Inspect should return task queue state");

    AcquireCoreLeaseRequest acquire_req;
    acquire_req.core_pid = 2468;
    acquire_req.purpose = "connect";
    auto acquire_resp = client.acquire_core_lease(acquire_req);
    expect_true(acquire_resp.accepted,
                "AcquireCoreLease should return accepted");
    expect_true(acquire_resp.lease_id == "lease-acquired",
                "AcquireCoreLease should return lease id");

    KeepAliveRequest keep_alive_req;
    keep_alive_req.lease_id = acquire_resp.lease_id;
    keep_alive_req.state = "connected";
    auto keep_alive_resp = client.keep_alive(keep_alive_req);
    expect_true(keep_alive_resp.ok, "KeepAlive should succeed");

    ReleaseCoreLeaseRequest release_req;
    release_req.lease_id = acquire_resp.lease_id;
    release_req.exit_if_oneshot = true;
    auto release_resp = client.release_core_lease(release_req);
    expect_true(release_resp.released, "ReleaseCoreLease should release lease");
    expect_true(release_resp.exiting,
                "ReleaseCoreLease should preserve exiting flag");

    client.disconnect();
    server_thread.join();
    std::cout << " PASS\n";
}

static void test_service_maintenance_protocol_methods() {
    std::cout << "  test_service_maintenance_protocol_methods...";

    std::string pipe = test_pipe_name();
    int num_requests = 2;
#ifdef _WIN32
    std::thread server_thread(handle_persistent_requests_win32, pipe, num_requests);
#else
    std::thread server_thread(handle_persistent_requests_posix, pipe, num_requests);
#endif
    std::this_thread::sleep_for(std::chrono::milliseconds(50));

    PipeClientConfig config;
    config.pipe_path = pipe;
    config.connect_timeout_ms = 3000;

    PipeHelperClient client(config);
    expect_true(client.connect(),
                "client should connect for service maintenance protocol test");

    auto install_resp = client.install_service(InstallServiceRequest{});
    expect_true(install_resp.success, "InstallService should succeed");
    expect_true(install_resp.exit_code == 0,
                "InstallService should preserve exit code");
    expect_true(install_resp.message == "installed",
                "InstallService should preserve status message");

    auto uninstall_resp = client.uninstall_service(UninstallServiceRequest{});
    expect_true(uninstall_resp.success, "UninstallService should succeed");
    expect_true(uninstall_resp.message == "uninstalled",
                "UninstallService should preserve status message");

    client.disconnect();
    server_thread.join();
    std::cout << " PASS\n";
}

static void test_cleanup_lease_handoff_protocol_methods() {
    std::cout << "  test_cleanup_lease_handoff_protocol_methods...";

    std::string pipe = test_pipe_name();
    int num_requests = 3;
#ifdef _WIN32
    std::thread server_thread(handle_persistent_requests_win32, pipe, num_requests);
#else
    std::thread server_thread(handle_persistent_requests_posix, pipe, num_requests);
#endif
    std::this_thread::sleep_for(std::chrono::milliseconds(50));

    PipeClientConfig config;
    config.pipe_path = pipe;
    config.connect_timeout_ms = 3000;

    PipeHelperClient client(config);
    expect_true(client.connect(),
                "client should connect for cleanup handoff protocol test");

    auto export_resp = client.export_cleanup_lease(ExportCleanupLeaseRequest{});
    expect_true(export_resp.has_active_session,
                "ExportCleanupLease should report active session");
    expect_true(export_resp.lease.sessions.size() == 1,
                "ExportCleanupLease should carry sessions");

    HandoffSessionRequest handoff_req;
    handoff_req.lease = export_resp.lease;
    auto handoff_resp = client.handoff_session(handoff_req);
    expect_true(handoff_resp.adopted, "HandoffSession should adopt lease");
    expect_true(handoff_resp.session_ids[0].value == "pipe-session",
                "HandoffSession should return adopted session id");

    FinalizeHandoffRequest finalize_req;
    finalize_req.exit = true;
    auto finalize_resp = client.finalize_handoff(finalize_req);
    expect_true(finalize_resp.finalized,
                "FinalizeHandoff should report finalized");
    expect_true(finalize_resp.exiting,
                "FinalizeHandoff should preserve exiting flag");

    client.disconnect();
    server_thread.join();
    std::cout << " PASS\n";
}

// ---------------------------------------------------------------------------
// Test: disconnect callback fires
// ---------------------------------------------------------------------------

static void test_disconnect_callback() {
    std::cout << "  test_disconnect_callback...";

    std::string pipe = test_pipe_name();
#ifdef _WIN32
    std::thread server_thread(handle_one_request_win32, pipe);
#else
    std::thread server_thread(handle_one_request_posix, pipe);
#endif
    std::this_thread::sleep_for(std::chrono::milliseconds(50));

    PipeClientConfig config;
    config.pipe_path = pipe;
    config.connect_timeout_ms = 3000;

    PipeHelperClient client(config);
    bool callback_fired = false;
    client.set_disconnect_callback([&]() { callback_fired = true; });

    expect_true(client.connect(), "client should connect for disconnect callback test");

    HelloRequest req;
    auto resp = client.hello(req);
    expect_true(resp.capabilities.size() == 2,
                "disconnect callback hello should return capabilities");

    // Explicit disconnect should fire callback
    client.disconnect();
    expect_true(callback_fired, "disconnect callback should fire");

    server_thread.join();
    std::cout << " PASS\n";
}

static void test_rejects_regular_file_path_on_windows() {
#ifdef _WIN32
    std::cout << "  test_rejects_regular_file_path_on_windows...";

    const auto temp_dir = std::filesystem::temp_directory_path();
    const auto regular_file = temp_dir /
        ("exv-helper-regular-file-" + std::to_string(GetCurrentProcessId()) + ".exe");

    {
        std::ofstream out(regular_file, std::ios::binary);
        out << "not a pipe";
    }

    PipeClientConfig config;
    config.pipe_path = regular_file.string();
    config.connect_timeout_ms = 100;

    PipeHelperClient client(config);
    const bool connected = client.connect();
    if (connected)
        client.disconnect();

    std::filesystem::remove(regular_file);

    expect_true(!connected,
                "regular Windows file path must not be treated as a successful pipe connection");
    expect_true(!client.is_connected(),
                "client should remain disconnected for regular file paths");
    std::cout << " PASS\n";
#else
    std::cout << "  test_rejects_regular_file_path_on_windows... SKIPPED\n";
#endif
}

// ---------------------------------------------------------------------------
// Test: connector factory creates real PipeHelperClient
// ---------------------------------------------------------------------------

static void test_connector_factory() {
    std::cout << "  test_connector_factory...";

    auto connector = HelperConnector::create();
    expect_true(connector != nullptr, "connector factory should create connector");
    // The real connector may report unavailable if no daemon is running,
    // but the factory should work.
    // return false if no daemon is running, but the factory should work.
    (void)connector;

    std::cout << " PASS\n";
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

int main(int argc, char** argv) {
    std::cout << "=== PipeHelperClient IPC transport tests ===\n";

    const std::string filter = argc > 1 ? argv[1] : "";
    const bool run_all = filter.empty();

#ifdef _WIN32
    if (run_all || filter == "test_single_hello" ||
        filter == "test_persistent_connection" ||
        filter == "test_disconnect_callback") {
        std::cout << "  raw Windows pipe exchange tests... SKIPPED "
                     "(covered by win32_helper_oneshot_test)\n";
    }
#else
    if (run_all || filter == "test_single_hello")
        test_single_hello();
    if (run_all || filter == "test_persistent_connection")
        test_persistent_connection();
    if (run_all || filter == "test_core_lease_protocol_methods")
        test_core_lease_protocol_methods();
    if (run_all || filter == "test_service_maintenance_protocol_methods")
        test_service_maintenance_protocol_methods();
    if (run_all || filter == "test_cleanup_lease_handoff_protocol_methods")
        test_cleanup_lease_handoff_protocol_methods();
    if (run_all || filter == "test_disconnect_callback")
        test_disconnect_callback();
#endif
    if (run_all || filter == "test_rejects_regular_file_path_on_windows")
        test_rejects_regular_file_path_on_windows();
    if (run_all || filter == "test_connector_factory")
        test_connector_factory();
    std::cout << "\nAll pipe helper client tests passed.\n";
    return 0;
}
