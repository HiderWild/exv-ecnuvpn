#pragma once

#include "windows_setup_rust/progress_model.hpp"

#include <cstdint>
#include <string>

namespace exv::setup {

struct UninstallRequest {
  std::wstring install_dir;  // empty → read registry
  bool clear_user_data{false};
  std::wstring user_profile_root;
  std::wstring local_app_data_root;
  std::wstring roaming_app_data_root;
  std::wstring config_dir;
  std::wstring temp_root;
  bool include_credential_manager{true};
  std::wstring credential_cleanup_note;
};

struct UninstallResult {
  bool ok{false};
  bool reboot_required{false};
  std::uint32_t scheduled_delete_count{0};
  std::wstring error;
  std::wstring notice;
  std::wstring diagnostic_log_path;
};

UninstallResult RunUninstall(const UninstallRequest &request, ProgressModel *progress = nullptr);

// User-facing detail shared by Done and Error pages. Preserves non-fatal notices,
// diagnostics and reboot-deletion facts even when another uninstall step failed.
std::wstring BuildUninstallResultDetail(const UninstallResult &result);

}  // namespace exv::setup
