#pragma once
#include <memory>
#include <string>

namespace exv::helper {

class HelperClient;

enum class ConnectorMode {
    Transient,   // Launch helper on demand, exits when done
    Resident     // Connect to installed service
};

struct HelperConnectorConfig {
    ConnectorMode mode = ConnectorMode::Transient;
    std::string helper_executable_path;
    int connect_timeout_ms = 5000;
    int heartbeat_interval_ms = 10000;
};

class HelperConnector {
public:
    virtual ~HelperConnector() = default;
    virtual std::unique_ptr<HelperClient> connect(const HelperConnectorConfig& config) = 0;
    virtual bool is_helper_available() const = 0;

    // Platform factory
    static std::unique_ptr<HelperConnector> create();
};

} // namespace exv::helper
