#include "fake_core_ui_client.hpp"

namespace exv::test {

void FakeCoreUiClient::on_status_update(const core::TunnelStatusSnapshot& snapshot) {
    snapshots_.push_back(snapshot);
}

std::vector<core::TunnelStatusSnapshot> FakeCoreUiClient::received_snapshots() const {
    return snapshots_;
}

core::TunnelStatusSnapshot FakeCoreUiClient::last_snapshot() const {
    return snapshots_.empty() ? core::TunnelStatusSnapshot{} : snapshots_.back();
}

int FakeCoreUiClient::snapshot_count() const {
    return snapshots_.size();
}

void FakeCoreUiClient::clear() {
    snapshots_.clear();
}

} // namespace exv::test
