#include "cleanup_registry.hpp"

#include <nlohmann/json.hpp>
#include <fstream>

namespace exv::helper {

namespace {

nlohmann::json route_entry_to_json(const RouteEntry& r) {
    return {{"destination", r.destination},
            {"gateway", r.gateway},
            {"metric", r.metric}};
}

RouteEntry json_to_route_entry(const nlohmann::json& j) {
    RouteEntry r;
    r.destination = j.value("destination", "");
    r.gateway = j.value("gateway", "");
    r.metric = j.value("metric", 0);
    return r;
}

nlohmann::json dns_config_to_json(const DnsConfig& d) {
    return {{"servers", d.servers}, {"search_domain", d.search_domain}};
}

DnsConfig json_to_dns_config(const nlohmann::json& j) {
    DnsConfig d;
    if (j.contains("servers")) d.servers = j["servers"].get<std::vector<std::string>>();
    d.search_domain = j.value("search_domain", "");
    return d;
}

nlohmann::json managed_resource_to_json(const ManagedResource& r) {
    return {{"type", r.type}, {"detail", r.detail}};
}

ManagedResource json_to_managed_resource(const nlohmann::json& j) {
    ManagedResource r;
    r.type = j.value("type", "");
    r.detail = j.value("detail", "");
    return r;
}

nlohmann::json core_registry_cleanup_to_json(
    const CoreRegistryCleanupBinding& binding) {
    return {{"registry_path", binding.registry_path},
            {"delete_match",
             {{"core_instance_id", binding.delete_match.core_instance_id},
              {"pid", binding.delete_match.pid},
              {"helper_core_lease_id",
               binding.delete_match.helper_core_lease_id},
              {"ipc_protocol_version",
               binding.delete_match.ipc_protocol_version}}}};
}

std::optional<CoreRegistryCleanupBinding> json_to_core_registry_cleanup(
    const nlohmann::json& j) {
    try {
        CoreRegistryCleanupBinding binding;
        binding.registry_path = j.at("registry_path").get<std::string>();
        const auto& match = j.at("delete_match");
        binding.delete_match.core_instance_id =
            match.at("core_instance_id").get<std::string>();
        binding.delete_match.pid = match.at("pid").get<int>();
        binding.delete_match.helper_core_lease_id =
            match.at("helper_core_lease_id").get<std::string>();
        binding.delete_match.ipc_protocol_version =
            match.at("ipc_protocol_version").get<std::string>();
        return binding;
    } catch (...) {
        return std::nullopt;
    }
}

nlohmann::json cleanup_record_to_json(const CleanupRecord& rec) {
    nlohmann::json routes = nlohmann::json::array();
    for (const auto& r : rec.routes) routes.push_back(route_entry_to_json(r));

    nlohmann::json fw = nlohmann::json::array();
    for (const auto& f : rec.firewall_rules) fw.push_back(f);

    nlohmann::json managed_resources = nlohmann::json::array();
    for (const auto& resource : rec.managed_resources) {
        managed_resources.push_back(managed_resource_to_json(resource));
    }

    auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(
        rec.created_at.time_since_epoch()).count();

    return {{"session_id", rec.session_id.value},
            {"adapter_name", rec.adapter_name},
            {"routes", routes},
            {"dns", dns_config_to_json(rec.dns)},
            {"firewall_rules", fw},
            {"managed_resources", managed_resources},
            {"created_at_ms", ms},
            {"core_registry_cleanup",
             rec.core_registry_cleanup.has_value()
                 ? core_registry_cleanup_to_json(*rec.core_registry_cleanup)
                 : nlohmann::json(nullptr)}};
}

CleanupRecord json_to_cleanup_record(const nlohmann::json& j) {
    CleanupRecord rec;
    rec.session_id.value = j.value("session_id", "");
    rec.adapter_name = j.value("adapter_name", "");
    if (j.contains("routes")) {
        for (const auto& rj : j["routes"])
            rec.routes.push_back(json_to_route_entry(rj));
    }
    if (j.contains("dns")) rec.dns = json_to_dns_config(j["dns"]);
    if (j.contains("firewall_rules"))
        rec.firewall_rules = j["firewall_rules"].get<std::vector<std::string>>();
    if (j.contains("managed_resources")) {
        for (const auto& resource : j["managed_resources"]) {
            rec.managed_resources.push_back(json_to_managed_resource(resource));
        }
    }
    if (j.contains("created_at_ms")) {
        auto ms = j["created_at_ms"].get<int64_t>();
        rec.created_at = std::chrono::system_clock::time_point(
            std::chrono::milliseconds(ms));
    }
    if (j.contains("core_registry_cleanup") &&
        !j["core_registry_cleanup"].is_null()) {
        rec.core_registry_cleanup =
            json_to_core_registry_cleanup(j["core_registry_cleanup"]);
    }
    return rec;
}

} // anonymous namespace

void CleanupRegistry::register_session(const CleanupRecord& record) {
    records_[record.session_id] = record;
}

void CleanupRegistry::add_resource(const SessionId& id, const ManagedResource& resource) {
    auto it = records_.find(id);
    if (it == records_.end()) return;
    it->second.managed_resources.push_back(resource);
}

void CleanupRegistry::bind_core_registry_cleanup(
    const SessionId& id, const CoreRegistryCleanupBinding& binding) {
    auto it = records_.find(id);
    if (it == records_.end()) {
        return;
    }
    it->second.core_registry_cleanup = binding;
}

std::vector<ManagedResource> CleanupRegistry::get_resources(const SessionId& id) const {
    auto it = records_.find(id);
    if (it == records_.end()) return {};
    std::vector<ManagedResource> result;
    // Reconstruct from CleanupRecord fields
    for (const auto& r : it->second.routes) {
        result.push_back({"route", r.destination + " via " + r.gateway});
    }
    for (const auto& s : it->second.dns.servers) {
        result.push_back({"dns", s});
    }
    for (const auto& f : it->second.firewall_rules) {
        result.push_back({"firewall_rule", f});
    }
    if (!it->second.adapter_name.empty()) {
        result.push_back({"adapter", it->second.adapter_name});
    }
    result.insert(result.end(),
                  it->second.managed_resources.begin(),
                  it->second.managed_resources.end());
    return result;
}

void CleanupRegistry::remove_session(const SessionId& id) {
    auto it = records_.find(id);
    if (it == records_.end()) {
        return;
    }
    records_.erase(it);
}

void CleanupRegistry::complete_session_cleanup(const SessionId& id) {
    auto it = records_.find(id);
    if (it == records_.end()) {
        return;
    }
    if (it->second.core_registry_cleanup.has_value()) {
        const auto& binding = *it->second.core_registry_cleanup;
        (void)exv::core::lifecycle::compare_and_delete_core_registry(
            binding.registry_path, binding.delete_match);
    }
    records_.erase(it);
}

std::vector<CleanupRecord> CleanupRegistry::all_records() const {
    std::vector<CleanupRecord> result;
    result.reserve(records_.size());
    for (const auto& [_, rec] : records_) {
        result.push_back(rec);
    }
    return result;
}

bool CleanupRegistry::save_to_disk(const std::string& path) const {
    try {
        nlohmann::json arr = nlohmann::json::array();
        for (const auto& [_, rec] : records_) {
            arr.push_back(cleanup_record_to_json(rec));
        }
        std::ofstream ofs(path);
        if (!ofs.is_open()) return false;
        ofs << arr.dump(2);
        return ofs.good();
    } catch (...) {
        return false;
    }
}

bool CleanupRegistry::load_from_disk(const std::string& path) {
    try {
        std::ifstream ifs(path);
        if (!ifs.is_open()) return false;
        nlohmann::json arr = nlohmann::json::parse(ifs);
        if (!arr.is_array()) return false;
        records_.clear();
        for (const auto& j : arr) {
            auto rec = json_to_cleanup_record(j);
            records_[rec.session_id] = std::move(rec);
        }
        return true;
    } catch (...) {
        return false;
    }
}

} // namespace exv::helper
