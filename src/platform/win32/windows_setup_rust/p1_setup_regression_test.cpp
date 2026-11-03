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
  Expect(exv::setup::ResolveDefaultInstallDir(L"D:\\Apps\\EXV", L"C:\\Users\\User\\AppData\\Local\\Programs\\EXV") ==
             L"D:\\Apps\\EXV",
         "default install directory preserves an existing registered custom path");
  Expect(exv::setup::ResolveDefaultInstallDir(L"", L"C:\\Users\\User\\AppData\\Local\\Programs\\EXV") ==
             L"C:\\Users\\User\\AppData\\Local\\Programs\\EXV",
         "default install directory uses per-user fallback for a fresh install");

  {
    auto storage = MakeArgs({L"exv-setup.exe"});
    auto pointers = Pointers(storage);
    const auto options = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(options.status == exv::setup::CliParseStatus::Ok, "default install parse succeeds");
    if (options.status == exv::setup::CliParseStatus::Ok) {
      Expect(exv::setup::ResolveGuiInstallDir(false, options.options) == exv::setup::DefaultInstallDir(),
             "install GUI keeps default directory behavior");
    }
  }

  {
    auto storage = MakeArgs({L"Uninstall.exe", L"/uninstall"});
    auto pointers = Pointers(storage);
    const auto options = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(options.status == exv::setup::CliParseStatus::Ok, "uninstall GUI parse succeeds");
    if (options.status == exv::setup::CliParseStatus::Ok) {
      Expect(exv::setup::ResolveGuiInstallDir(true, options.options).empty(),
             "uninstall GUI without /D preserves empty directory for registry lookup");
    }
  }

  {
    auto storage = MakeArgs({L"Uninstall.exe", L"/uninstall", L"/D=C:\\Custom\\EXV"});
    auto pointers = Pointers(storage);
    const auto options = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(options.status == exv::setup::CliParseStatus::Ok, "explicit uninstall directory parse succeeds");
    if (options.status == exv::setup::CliParseStatus::Ok) {
      Expect(exv::setup::ResolveGuiInstallDir(true, options.options) == L"C:\\Custom\\EXV",
             "uninstall GUI preserves explicit /D directory");
    }
  }

  {
    exv::setup::CliOptions source;
    source.role = exv::setup::Role::UninstallGui;
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
      Expect(parsed.status == exv::setup::CliParseStatus::Ok,
             "elevation context round-trip parses");
      if (parsed.status == exv::setup::CliParseStatus::Ok) {
        Expect(parsed.options.role == exv::setup::Role::UninstallGui,
               "uninstall GUI role survives elevation round-trip (S1: no silent role)");
        Expect(!parsed.options.clear_user_data,
               "GUI elevation may precede selecting clear-user-data");
        Expect(parsed.options.install_dir == source.install_dir,
               "install directory with spaces and trailing slash survives quoting");
        Expect(parsed.options.initiating_user_context, "initiating context marker survives elevation");
        Expect(parsed.options.initiating_user_sid == source.initiating_user_sid,
               "initiating SID survives elevation round-trip");
        Expect(parsed.options.user_profile_root == source.user_profile_root,
               "USERPROFILE survives elevation round-trip");
        Expect(parsed.options.local_app_data_root == source.local_app_data_root,
               "LOCALAPPDATA with trailing slash survives elevation round-trip");
        Expect(parsed.options.roaming_app_data_root == source.roaming_app_data_root,
               "APPDATA survives elevation round-trip");
        Expect(parsed.options.config_dir == source.config_dir,
               "EXV_CONFIG_DIR with trailing slash survives elevation round-trip");
        Expect(parsed.options.temp_root == source.temp_root,
               "original-user TEMP with trailing slash survives elevation round-trip");
      }
    }
  }

  {
    // KI-13 ④：升级链路语义开关 /preserve-user-data 的解析与 UAC 参数往返。
    auto storage = MakeArgs({L"Uninstall.exe", L"/uninstall",
                             L"/clear-user-data", L"/preserve-user-data"});
    auto pointers = Pointers(storage);
    const auto options = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(options.status == exv::setup::CliParseStatus::Ok,
           "upgrade-mode uninstall parse succeeds");
    if (options.status == exv::setup::CliParseStatus::Ok) {
      Expect(options.options.role == exv::setup::Role::UninstallGui,
             "upgrade-mode uninstall resolves the uninstall GUI role");
      Expect(options.options.clear_user_data, "upgrade-mode uninstall keeps the clear-data run");
      Expect(options.options.preserve_user_data,
             "upgrade-mode uninstall parses the preserve-user-data switch");
    }

    exv::setup::CliOptions source;
    source.role = exv::setup::Role::UninstallGui;
    source.clear_user_data = true;
    source.preserve_user_data = true;
    source.install_dir = L"C:\\Program Files\\EXV";
    source.initiating_user_context = true;
    source.user_profile_root = L"C:\\Users\\Original User";

    const std::wstring command_line =
        L"Uninstall.exe " + exv::setup::BuildUninstallElevationParameters(source);
    int argc = 0;
    LPWSTR *argv = CommandLineToArgvW(command_line.c_str(), &argc);
    Expect(argv != nullptr, "upgrade elevation command line parses with CommandLineToArgvW");
    if (argv) {
      const auto parsed = exv::setup::ParseCli(argc, argv);
      LocalFree(argv);
      Expect(parsed.status == exv::setup::CliParseStatus::Ok,
             "upgrade elevation context round-trip parses");
      if (parsed.status == exv::setup::CliParseStatus::Ok) {
        Expect(parsed.options.clear_user_data && parsed.options.preserve_user_data,
               "upgrade semantics survive the UAC round-trip");
      }
    }
  }

  // S1（2026-10-10 静默移除）严格白名单——正用例：内部提权 worker 旗标组合放行。
  {
    auto storage = MakeArgs({L"exv-setup.exe", L"/elevated-worker",
                             L"--pipe", L"\\\\.\\pipe\\exv-elev-1", L"--token", L"deadbeef"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::Ok,
           "elevated worker flag combination (space form) is whitelisted");
    if (parsed.status == exv::setup::CliParseStatus::Ok) {
      Expect(parsed.options.role == exv::setup::Role::ElevatedWorker,
             "worker flag combination resolves the ElevatedWorker role");
      Expect(parsed.options.elevated_pipe == L"\\\\.\\pipe\\exv-elev-1",
             "worker pipe name survives parsing");
      Expect(parsed.options.elevated_token == L"deadbeef", "worker token survives parsing");
    }
  }

  {
    // RunElevatedPfroSweep 生成形态：worker + PFRO 目录（elevate.cpp:425-427）。
    auto storage = MakeArgs({L"exv-setup.exe", L"/elevated-worker",
                             L"--pipe=\\\\.\\pipe\\exv-elev-2-pfro", L"--token=cafe",
                             L"--pfro-sweep-dir=C:\\Install\\EXV"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::Ok,
           "pfro sweep worker flag combination (= form) is whitelisted");
    if (parsed.status == exv::setup::CliParseStatus::Ok) {
      Expect(parsed.options.role == exv::setup::Role::ElevatedWorker,
             "pfro worker resolves the ElevatedWorker role");
      Expect(parsed.options.elevated_pfro_sweep_dir == L"C:\\Install\\EXV",
             "pfro sweep directory survives parsing");
    }
  }

  {
    // 卡片裁定：提权旗标独立传入无害（无 --elevated-worker 时保持 GUI 安装角色）。
    auto storage = MakeArgs({L"exv-setup.exe", L"--pipe", L"name", L"--token", L"tok",
                             L"--pfro-sweep-dir", L"C:\\Temp"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::Ok,
           "standalone worker flags without --elevated-worker stay harmless");
    if (parsed.status == exv::setup::CliParseStatus::Ok) {
      Expect(parsed.options.role == exv::setup::Role::InstallGui,
             "standalone worker flags keep the install GUI role");
    }
  }

  {
    // 安装器 /D= 与 /D 空格两形态仍放行。
    auto storage = MakeArgs({L"exv-setup.exe", L"/D=C:\\Apps\\EXV"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::Ok, "installer /D= form is whitelisted");
    if (parsed.status == exv::setup::CliParseStatus::Ok) {
      Expect(parsed.options.install_dir == L"C:\\Apps\\EXV", "installer /D= value survives");
      Expect(parsed.options.role == exv::setup::Role::InstallGui, "installer /D= keeps GUI role");
    }
  }

  // S1 严格白名单——负用例：白名单外参数一律 InvalidArgument（退出码 3）。
  {
    auto storage = MakeArgs({L"exv-setup.exe", L"/S"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::InvalidArgument,
           "installer /S is rejected (silent install removed)");
    Expect(parsed.rejected_argument == L"/S", "installer /S rejection names the argument");
  }

  {
    auto storage = MakeArgs({L"Uninstall.exe", L"/uninstall", L"/S"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::InvalidArgument,
           "uninstaller /S is rejected (silent uninstall removed)");
    Expect(parsed.rejected_argument == L"/S", "uninstaller /S rejection names the argument");
  }

  {
    auto storage = MakeArgs({L"exv-setup.exe", L"/silent"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::InvalidArgument,
           "/silent alias is rejected");
  }

  {
    auto storage = MakeArgs({L"exv-setup.exe", L"--silent"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::InvalidArgument,
           "--silent alias is rejected");
  }

  {
    auto storage = MakeArgs({L"exv-setup.exe", L"/desktop-shortcut=false"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::InvalidArgument,
           "boolean flag override is rejected (silent-only surface removed)");
  }

  {
    auto storage = MakeArgs({L"exv-setup.exe", L"/launch=false"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::InvalidArgument,
           "/launch override is rejected");
  }

  {
    auto storage = MakeArgs({L"exv-setup.exe", L"--unknown-flag"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::InvalidArgument,
           "unknown --flag is rejected");
    Expect(parsed.rejected_argument == L"--unknown-flag", "unknown flag rejection names it");
  }

  {
    // 白名单外的裸位置参数同样拒绝（fail-closed，不再静默忽略）。
    auto storage = MakeArgs({L"exv-setup.exe", L"stray-argument"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::InvalidArgument,
           "positional argument is rejected");
    Expect(parsed.rejected_argument == L"stray-argument",
           "positional rejection names the argument");
  }

  {
    // 卸载器白名单外：提权 worker 旗标不得混入卸载器。
    auto storage = MakeArgs({L"Uninstall.exe", L"/uninstall", L"--pipe", L"name"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::InvalidArgument,
           "worker flags are out of the uninstaller whitelist");
    Expect(parsed.rejected_argument == L"--pipe",
           "uninstaller rejection names the worker flag");
  }

  {
    // 安装器白名单外：卸载器专属旗标不得混入安装器。
    auto storage = MakeArgs({L"exv-setup.exe", L"/clear-user-data"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::InvalidArgument,
           "uninstall-only flags are out of the installer whitelist");
    Expect(parsed.rejected_argument == L"/clear-user-data",
           "installer rejection names the uninstall-only flag");
  }

  {
    // worker 白名单外：worker 与卸载旗标混用拒绝，报告命令行首个违规参数。
    auto storage = MakeArgs({L"exv-setup.exe", L"/elevated-worker", L"--pipe", L"n",
                             L"--token", L"t", L"/uninstall"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::InvalidArgument,
           "worker cannot mix uninstall flags");
    Expect(parsed.rejected_argument == L"/uninstall",
           "mixed rejection names the earliest out-of-whitelist argument");
  }

  {
    // worker 白名单外：/D 不得混入 worker。
    auto storage = MakeArgs({L"exv-setup.exe", L"/D=C:\\X", L"/elevated-worker",
                             L"--pipe", L"n", L"--token", L"t"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::InvalidArgument,
           "worker cannot mix /D");
    Expect(parsed.rejected_argument == L"/D=C:\\X",
           "worker /D rejection reports the earlier argument");
  }

  {
    // 既有早退失败语义（退出码 2）不动：worker 缺管道/令牌为 Fatal，非参数非法。
    auto storage = MakeArgs({L"exv-setup.exe", L"/elevated-worker"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::Fatal,
           "worker without pipe/token stays a fatal parse error (exit 2)");
  }

  {
    // 卸载器全量 runas 往返七旗标组合放行（生成侧完备枚举）。
    auto storage = MakeArgs({L"Uninstall.exe", L"/uninstall", L"/preserve-user-data",
                             L"/D=C:\\Program Files\\EXV", L"--initiating-user-context",
                             L"--initiating-user-sid", L"S-1-5-21-7",
                             L"--user-profile-root", L"C:\\Users\\U",
                             L"--local-app-data-root", L"C:\\Users\\U\\AppData\\Local",
                             L"--roaming-app-data-root", L"C:\\Users\\U\\AppData\\Roaming",
                             L"--config-dir", L"D:\\Cfg", L"--temp-root", L"C:\\Temp"});
    auto pointers = Pointers(storage);
    const auto parsed = exv::setup::ParseCli(static_cast<int>(pointers.size()), pointers.data());
    Expect(parsed.status == exv::setup::CliParseStatus::Ok,
           "uninstaller runas round-trip flag set is fully whitelisted");
    if (parsed.status == exv::setup::CliParseStatus::Ok) {
      Expect(parsed.options.role == exv::setup::Role::UninstallGui,
             "full runas flag set keeps the uninstall GUI role");
      Expect(parsed.options.initiating_user_sid == L"S-1-5-21-7",
             "runas SID survives the full whitelist parse");
    }
  }

  if (failures != 0) {
    return 1;
  }
  std::cout << "p1_setup_regression_test: ok\n";
  return 0;
}
