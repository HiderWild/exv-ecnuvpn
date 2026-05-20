#pragma once

#include <nlohmann/json.hpp>

#include <functional>
#include <string>

namespace ecnuvpn {
namespace helper {
class IpcServer;
} // namespace helper

namespace platform {

void cleanup_routes();
void kill_all_supervisors();
void fix_config_dir_ownership();
int copy_self_to_stable_path_and_reexec(const std::string &current_path);

// Check if a process with the given PID is still alive.
bool is_process_alive(int pid);

// Find the PID of the openconnect process. Returns -1 if not found.
int find_openconnect_pid();

// Get the network interfaces output string for the current platform.
std::string get_interfaces_output();

// Create a temporary file containing `payload` and return its path.
// Returns empty string on failure.
std::string create_temp_request_file(const std::string &payload);

// Spawn a worker process to handle a helper request.
// Returns 0 on success, non-zero on failure.
int spawn_worker_process(const std::string &executable_path,
                         const std::string &request_path);

// Terminate a process by PID. Used for stopping VPN/supervisor processes.
void terminate_process(int pid);

// Sleep for the given number of milliseconds.
void sleep_ms(int milliseconds);

// Reap finished child processes (POSIX only; no-op on Windows).
void reap_children();

// Set restrictive permissions on the session state file (POSIX only; no-op on Windows).
void set_session_state_permissions(const std::string &path);

// Set up platform-specific daemon signal handlers (e.g. SIGPIPE on POSIX).
void setup_daemon_signals();

// Clean up the daemon IPC endpoint (e.g. remove Unix socket on POSIX; no-op on Windows).
void cleanup_daemon_endpoint(const std::string &endpoint);

// Dispatch a helper request in the background (fork on POSIX, thread on Windows).
// The handler callback produces a JSON response string.
// On POSIX, the caller must close the client fd after this returns.
// On Windows, the thread handles everything including response sending.
void dispatch_request_background(
    helper::IpcServer &ipc, const std::string &raw_request,
    unsigned int peer_uid, unsigned int peer_gid,
    std::function<nlohmann::json(unsigned int, unsigned int,
                                  const nlohmann::json &)> handler);

} // namespace platform
} // namespace ecnuvpn
