#include "windows_setup_rust/cli.hpp"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shellapi.h>

#include <iostream>
#include <string>
#include <vector>

namespace {

int failures = 0;

void Expect(bool condition, const char *message) {
  if (!condition) {
    std::cerr << "EXPECT FAILED: " << message << '\n';
    ++failures;
  }
}

std::vector<std::wstring> MakeArgs(std::initializer_list<const wchar_t *> parts) {
  std::vector<std::wstring> storage;
  storage.reserve(parts.size());
  for (const auto *part : parts) {
    storage.emplace_back(part);
  }
  return storage;
}

std::vector<wchar_t *> Pointers(std::vector<std::wstring> &storage) {
  std::vector<wchar_t *> pointers;
  pointers.reserve(storage.size());
  for (auto &value : storage) {
    pointers.push_back(value.data());
  }
  return pointers;
}

}  // namespace

int main() {
  {
    auto storage = MakeArgs({L"exv-setup.exe"});
    auto pointers = Pointers(storage);
    const auto options = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(options.has_value(), "default install parse succeeds");
    if (options) {
      Expect(exv::setup::ResolveGuiInstallDir(false, *options) == exv::setup::DefaultInstallDir(),
             "install GUI keeps default directory behavior");
    }
  }

  {
    auto storage = MakeArgs({L"Uninstall.exe", L"/uninstall"});
    auto pointers = Pointers(storage);
    const auto options = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(options.has_value(), "uninstall GUI parse succeeds");
    if (options) {
      Expect(exv::setup::ResolveGuiInstallDir(true, *options).empty(),
             "uninstall GUI without /D preserves empty directory for registry lookup");
    }
  }

  {
    auto storage = MakeArgs({L"Uninstall.exe", L"/uninstall", L"/D=C:\\Custom\\EXV"});
    auto pointers = Pointers(storage);
    const auto options = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(options.has_value(), "explicit uninstall directory parse succeeds");
    if (options) {
      Expect(exv::setup::ResolveGuiInstallDir(true, *options) == L"C:\\Custom\\EXV",
             "uninstall GUI preserves explicit /D directory");
    }
  }

  {
    exv::setup::CliOptions source;
    source.role = exv::setup::Role::UninstallSilent;
    // GUI enters UAC before the checkbox is selected. Context must survive even
    // when clear_user_data is false at elevation time, then remain available if
    // the user checks it on the elevated confirmation page.
    source.clear_user_data = false;
    source.install_dir = L"C:\\Program Files\\EXV\\";
    source.initiating_user_context = true;
    source.initiating_user_sid = L"S-1-5-21-1000";
    source.user_profile_root = L"C:\\Users\\Original User";
    source.local_app_data_root = L"C:\\Users\\Original User\\AppData\\Local\\";
    source.roaming_app_data_root = L"C:\\Users\\Original User\\AppData\\Roaming";
    source.config_dir = L"D:\\EXV Config\\";
    source.temp_root = L"C:\\Users\\Original User\\AppData\\Local\\Temp\\";

    const std::wstring command_line =
        L"Uninstall.exe " + exv::setup::BuildUninstallElevationParameters(source);
    int argc = 0;
    LPWSTR *argv = CommandLineToArgvW(command_line.c_str(), &argc);
    Expect(argv != nullptr, "elevation command line parses with CommandLineToArgvW");
    if (argv) {
      const auto parsed = exv::setup::ParseCli(argc, argv);
      LocalFree(argv);
      Expect(parsed.has_value(), "elevation context round-trip parses");
      if (parsed) {
        Expect(parsed->role == exv::setup::Role::UninstallSilent,
               "silent uninstall role survives elevation round-trip");
        Expect(!parsed->clear_user_data,
               "GUI elevation may precede selecting clear-user-data");
        Expect(parsed->install_dir == source.install_dir,
               "install directory with spaces and trailing slash survives quoting");
        Expect(parsed->initiating_user_context, "initiating context marker survives elevation");
        Expect(parsed->initiating_user_sid == source.initiating_user_sid,
               "initiating SID survives elevation round-trip");
        Expect(parsed->user_profile_root == source.user_profile_root,
               "USERPROFILE survives elevation round-trip");
        Expect(parsed->local_app_data_root == source.local_app_data_root,
               "LOCALAPPDATA with trailing slash survives elevation round-trip");
        Expect(parsed->roaming_app_data_root == source.roaming_app_data_root,
               "APPDATA survives elevation round-trip");
        Expect(parsed->config_dir == source.config_dir,
               "EXV_CONFIG_DIR with trailing slash survives elevation round-trip");
        Expect(parsed->temp_root == source.temp_root,
               "original-user TEMP with trailing slash survives elevation round-trip");
      }
    }
  }

  if (failures != 0) {
    return 1;
  }
  std::cout << "p1_setup_regression_test: ok\n";
  return 0;
}
