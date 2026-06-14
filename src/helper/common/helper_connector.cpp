#include "helper_connector.hpp"
#include "helper_client.hpp"
#include "helper_messages.hpp"
#include "helper_error.hpp"
#include "pipe_helper_client.hpp"
#include "logger.hpp"

#include <stdexcept>

namespace exv::helper {

// ---------------------------------------------------------------------------
// PlatformHelperConnector -- production connector using named pipes / Unix sockets
// ---------------------------------------------------------------------------

class PlatformHelperConnector : public HelperConnector {
public:
    std::unique_ptr<HelperClient> connect(const HelperConnectorConfig& config) override {
        PipeClientConfig pc;
        pc.pipe_path = resolve_endpoint(config);
        pc.connect_timeout_ms = config.connect_timeout_ms;

        ecnuvpn::logger::info("Helper connector: Attempting connection - endpoint=" + 
                              pc.pipe_path + " timeout_ms=" + 
                              std::to_string(pc.connect_timeout_ms));

        auto client = std::make_unique<PipeHelperClient>(pc);
        if (!client->connect()) {
            ecnuvpn::logger::error("Helper connector: Connection failed - endpoint=" + pc.pipe_path);
            return nullptr;
        }
        
        ecnuvpn::logger::info("Helper connector: Connected successfully - endpoint=" + pc.pipe_path);
        return client;
    }

    bool is_helper_available() const override {
        // Best-effort: try connecting to the default endpoint.
        // Returns false if the daemon is not running.
        PipeClientConfig pc;
        pc.pipe_path = default_endpoint();
        pc.connect_timeout_ms = 500;  // quick probe
        PipeHelperClient probe(pc);
        return probe.connect();
    }

private:
    /// Determine the pipe / socket endpoint from the connector config.
    /// Priority: 1) explicit pipe_endpoint, 2) helper_executable_path if it
    /// looks like a pipe/socket, 3) platform default endpoint.
    static std::string resolve_endpoint(const HelperConnectorConfig& config) {
        // 1) Explicit pipe endpoint takes highest priority.
        if (!config.pipe_endpoint.empty()) {
            return config.pipe_endpoint;
        }

        // 2) helper_executable_path may carry a pipe/socket endpoint
        //    (legacy callers that don't set pipe_endpoint separately).
        if (!config.helper_executable_path.empty()) {
            const auto& p = config.helper_executable_path;
#ifdef _WIN32
            // On Windows, named pipes start with \\.\pipe\ or \\?\pipe\
            if (p.find("\\\\.\\pipe\\") == 0 || p.find("\\\\?\\pipe\\") == 0)
                return p;
#else
            // On POSIX, absolute paths are socket paths
            if (!p.empty() && p[0] == '/')
                return p;
#endif
        }
        return default_endpoint();
    }

    /// Platform-specific default endpoint for the Helper daemon.
    static std::string default_endpoint() {
#ifdef _WIN32
        return "\\\\.\\pipe\\exv-helper";
#elif defined(__APPLE__)
        return "/var/run/exv-helper.sock";
#else
        return "/var/run/exv-helper.sock";
#endif
    }
};

std::unique_ptr<HelperConnector> HelperConnector::create() {
    return std::make_unique<PlatformHelperConnector>();
}

} // namespace exv::helper
