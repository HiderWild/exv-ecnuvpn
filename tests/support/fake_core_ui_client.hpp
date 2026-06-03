#pragma once
#include "../../src/core/tunnel_state.hpp"
#include <vector>
#include <string>

namespace exv::test {

class FakeCoreUiClient {
public:
    // Simulate receiving status updates
    void on_status_update(const core::TunnelStatusSnapshot& snapshot);

    // Inspection
    std::vector<core::TunnelStatusSnapshot> received_snapshots() const;
    core::TunnelStatusSnapshot last_snapshot() const;
    int snapshot_count() const;

    // Reset
    void clear();

private:
    std::vector<core::TunnelStatusSnapshot> snapshots_;
};

} // namespace exv::test
