#pragma once
#include <string>
#include <functional>
#include <memory>

namespace exv::platform {

class IpcTransport {
public:
    virtual ~IpcTransport() = default;

    using MessageHandler = std::function<void(const std::string&)>;

    // Client side
    virtual bool connect(const std::string& endpoint) = 0;
    virtual bool send(const std::string& message) = 0;
    virtual void disconnect() = 0;
    virtual bool is_connected() const = 0;

    // Server side
    virtual bool listen(const std::string& endpoint) = 0;
    virtual void set_message_handler(MessageHandler handler) = 0;
    virtual void stop() = 0;

    // Platform factories
    static std::unique_ptr<IpcTransport> create_client();
    static std::unique_ptr<IpcTransport> create_server();
};

} // namespace exv::platform
