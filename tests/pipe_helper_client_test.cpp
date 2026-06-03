/// @file pipe_helper_client_test.cpp
/// @brief End-to-end test: PipeHelperClient <-> raw named pipe server.
///
/// This verifies that the V2 protocol can travel over the Windows named pipe
/// transport with a persistent connection, which is the path the Core process
/// will use to talk to the Helper daemon.

#include "helper_common/pipe_helper_client.hpp"
#include "helper_common/helper_connector.hpp"
#include "helper_common/helper_messages.hpp"
#include "helper_common/helper_protocol.hpp"

#include <nlohmann/json.hpp>

#include <cassert>
#include <iostream>
#include <string>
#include <thread>
#include <chrono>

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

/// Create a named pipe server, accept one client, handle one V2 hello request.
static bool handle_one_request_win32(const std::string& pipe_name) {
    HANDLE hPipe = CreateNamedPipeA(
        pipe_name.c_str(),
        PIPE_ACCESS_DUPLEX,
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
        1, 65536, 65536, 0, NULL);
    if (hPipe == INVALID_HANDLE_VALUE)
        return false;

    // Block until client connects
    if (!ConnectNamedPipe(hPipe, NULL)) {
        DWORD err = GetLastError();
        if (err != ERROR_PIPE_CONNECTED) {
            CloseHandle(hPipe);
            return false;
        }
    }

    // Read request (newline-delimited)
    std::string raw;
    char buffer[4096];
    DWORD bytesRead = 0;
    while (true) {
        if (!ReadFile(hPipe, buffer, sizeof(buffer) - 1, &bytesRead, NULL) || bytesRead == 0)
            break;
        raw.append(buffer, bytesRead);
        if (raw.find('\n') != std::string::npos)
            break;
    }

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
        hello_resp.server_version = PROTOCOL_VERSION;
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
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
        1, 65536, 65536, 0, NULL);
    if (hPipe == INVALID_HANDLE_VALUE)
        return false;

    if (!ConnectNamedPipe(hPipe, NULL)) {
        DWORD err = GetLastError();
        if (err != ERROR_PIPE_CONNECTED) {
            CloseHandle(hPipe);
            return false;
        }
    }

    for (int i = 0; i < num_requests; ++i) {
        // Read request
        std::string raw;
        char buffer[4096];
        DWORD bytesRead = 0;
        while (true) {
            if (!ReadFile(hPipe, buffer, sizeof(buffer) - 1, &bytesRead, NULL) || bytesRead == 0)
                break;
            raw.append(buffer, bytesRead);
            if (raw.find('\n') != std::string::npos)
                break;
        }
        if (raw.empty())
            break;

        json envelope = json::parse(raw);
        HelperRequest req = helper_request_from_json(envelope);

        HelperResponse resp;
        resp.op = req.op;
        resp.success = true;

        if (req.op == HelperOp::Hello) {
            HelloResponse hr;
            hr.server_version = PROTOCOL_VERSION;
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
        } else if (req.op == HelperOp::EndSession) {
            EndSessionResponse esr;
            esr.success = true;
            json payload;
            to_json(payload, esr);
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
        hr.server_version = PROTOCOL_VERSION;
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
            hr.server_version = PROTOCOL_VERSION;
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
        } else if (req.op == HelperOp::EndSession) {
            EndSessionResponse esr;
            esr.success = true;
            json payload;
            to_json(payload, esr);
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
    assert(client.connect());
    assert(client.is_connected());

    HelloRequest req;
    req.client_version = PROTOCOL_VERSION;
    auto resp = client.hello(req);

    assert(resp.server_version == PROTOCOL_VERSION);
    assert(resp.capabilities.size() == 2);
    assert(resp.mode == HelperMode::Transient);

    client.disconnect();
    assert(!client.is_connected());

    server_thread.join();
    std::cout << " PASS\n";
}

// ---------------------------------------------------------------------------
// Test: persistent connection (multiple V2 messages on one pipe)
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
    assert(client.connect());

    // 1. Hello
    HelloRequest hello_req;
    hello_req.client_version = PROTOCOL_VERSION;
    auto hello_resp = client.hello(hello_req);
    assert(hello_resp.server_version == PROTOCOL_VERSION);

    // 2. StartSession
    StartSessionRequest ss_req;
    ss_req.profile_id.value = "test-profile";
    auto ss_resp = client.start_session(ss_req);
    assert(!ss_resp.session_id.value.empty());

    // 3. Heartbeat
    HeartbeatRequest hb_req;
    hb_req.session_id = ss_resp.session_id;
    hb_req.core_phase = "Connected";
    auto hb_resp = client.heartbeat(hb_req);
    assert(hb_resp.ok);

    // 4. Another Heartbeat
    auto hb_resp2 = client.heartbeat(hb_req);
    assert(hb_resp2.ok);

    // 5. EndSession
    EndSessionRequest end_req;
    end_req.session_id = ss_resp.session_id;
    auto end_resp = client.end_session(end_req);
    assert(end_resp.success);

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

    assert(client.connect());

    HelloRequest req;
    req.client_version = PROTOCOL_VERSION;
    auto resp = client.hello(req);
    assert(resp.server_version == PROTOCOL_VERSION);

    // Explicit disconnect should fire callback
    client.disconnect();
    assert(callback_fired);

    server_thread.join();
    std::cout << " PASS\n";
}

// ---------------------------------------------------------------------------
// Test: connector factory creates real PipeHelperClient
// ---------------------------------------------------------------------------

static void test_connector_factory() {
    std::cout << "  test_connector_factory...";

    auto connector = HelperConnector::create();
    assert(connector != nullptr);
    // The real connector should NOT be a stub -- is_helper_available may
    // return false if no daemon is running, but the factory should work.
    (void)connector;

    std::cout << " PASS\n";
}

// ---------------------------------------------------------------------------
// Test: stub connector still works for unit tests
// ---------------------------------------------------------------------------

static void test_stub_connector_still_works() {
    std::cout << "  test_stub_connector_still_works...";

    auto connector = HelperConnector::create_stub();
    assert(connector != nullptr);
    assert(connector->is_helper_available());

    HelperConnectorConfig config;
    auto client = connector->connect(config);
    assert(client != nullptr);
    assert(client->is_connected());

    HelloRequest req;
    req.client_version = PROTOCOL_VERSION;
    auto resp = client->hello(req);
    assert(resp.server_version == PROTOCOL_VERSION);

    client->disconnect();
    assert(!client->is_connected());

    std::cout << " PASS\n";
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

int main() {
    std::cout << "=== PipeHelperClient IPC transport tests ===\n";

    test_single_hello();
    test_persistent_connection();
    test_disconnect_callback();
    test_connector_factory();
    test_stub_connector_still_works();

    std::cout << "\nAll pipe helper client tests passed.\n";
    return 0;
}
