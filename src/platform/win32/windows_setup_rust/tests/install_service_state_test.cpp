// 调用真实 RunInstall；仅替换 SCM、提权、注册表与快捷方式等宿主副作用。
// 文件落在当前测试构建目录的独有子目录，不操作已安装 EXV 或用户配置。
#include "windows_setup_rust/install_engine.cpp"
#include <iostream>

namespace exv::setup {
namespace fixture {
ServiceRuntimeState state;
std::vector<std::string> calls;
std::string fail_at;
bool Step(const char *name) {
  calls.emplace_back(name);
  return fail_at != name;
}
}
bool CanWriteDirectory(const std::wstring &) { return true; }
ServiceRuntimeSnapshot QueryServiceRuntimeState(const std::wstring &) {
  fixture::Step("query");
  return {fixture::state, fixture::state == ServiceRuntimeState::Running ? 4u : 1u, 0};
}
bool EnsureServiceStopped() {
  if (!fixture::Step("stop")) return false;
  if (fixture::state != ServiceRuntimeState::NotInstalled) fixture::state = ServiceRuntimeState::Stopped;
  return true;
}
bool EnsureServiceRunning(std::wstring *error) {
  if (!fixture::Step("start")) { *error = L"fixture start rejected"; return false; }
  if (fixture::state == ServiceRuntimeState::NotInstalled) return false;
  fixture::state = ServiceRuntimeState::Running;
  return true;
}
bool IsAnyExvProcessRunning() { return true; }
void StopRunningAppProcesses() {
  fixture::Step("process-stop");
  if (fixture::state != ServiceRuntimeState::NotInstalled) fixture::state = ServiceRuntimeState::Stopped;
}
UninstallerWriteResult WriteUninstallerCopy(const std::wstring &, const std::wstring &) {
  return {fixture::Step("uninstaller"), 0, L"fixture uninstaller rejected"};
}
bool WriteInstallRegistry(const std::wstring &, const std::wstring &, const std::wstring &) {
  return fixture::Step("registry");
}
bool CreateStartMenuShortcuts(const std::wstring &) { return fixture::Step("shortcuts"); }
bool CreateDesktopShortcut(const std::wstring &) { return fixture::Step("desktop"); }
bool CreateQuickLaunchShortcut(const std::wstring &) { return fixture::Step("quick-launch"); }
namespace payload {
bool ParseArchive(const std::vector<std::uint8_t> &, ArchiveView &) { return false; }
bool ExtractArchive(const ArchiveView &, const fs::path &, std::string *, const ProgressFn &) { return false; }
bool ExtractEmbeddedPayloadTo(const fs::path &destination, std::string *error, const ProgressFn &) {
  if (!fixture::Step("files")) { *error = "fixture extraction rejected"; return false; }
  fs::create_directories(destination);
  std::ofstream(destination / "payload.marker") << "test payload";
  return true;
}
}
}

#define CHECK(expression) do { if (!(expression)) { \
  std::cerr << "FAIL line " << __LINE__ << ": " #expression << '\n'; return 1; } } while (false)

int main() {
  using namespace exv::setup;
  const auto root = fs::current_path() / ("install-state-test-" + std::to_string(GetCurrentProcessId()));
  CHECK(!fs::exists(root));
  fs::create_directory(root);
  // 仅清理本测试刚创建、固定在当前构建目录下的独有子目录。
  struct Cleanup { fs::path path; ~Cleanup() { std::error_code ec; fs::remove_all(path, ec); } } cleanup{root};
  int case_id = 0;
  auto run = [&](ServiceRuntimeState initial, const std::string &failure = "") {
    fixture::state = initial;
    fixture::calls.clear();
    fixture::fail_at = failure;
    InstallRequest request;
    request.install_dir = (root / std::to_string(++case_id)).wstring();
    request.create_desktop_shortcut = false;
    return RunInstall(request);
  };
  auto seen = [](const char *step) {
    return std::find(fixture::calls.begin(), fixture::calls.end(), step) != fixture::calls.end();
  };
  CHECK(run(ServiceRuntimeState::Running).ok);
  CHECK(fixture::state == ServiceRuntimeState::Running);
  CHECK(fixture::calls == std::vector<std::string>({"query", "stop", "process-stop", "files", "uninstaller", "registry", "shortcuts", "start"}));
  CHECK(run(ServiceRuntimeState::Stopped).ok);
  CHECK(fixture::state == ServiceRuntimeState::Stopped);
  CHECK(!seen("start"));
  CHECK(run(ServiceRuntimeState::NotInstalled).ok);
  CHECK(fixture::state == ServiceRuntimeState::NotInstalled);
  CHECK(!seen("start"));
  for (const auto *failure : {"files", "uninstaller", "registry", "shortcuts"}) {
    const auto result = run(ServiceRuntimeState::Running, failure);
    CHECK(!result.ok && !result.error.empty());
    CHECK(fixture::state == ServiceRuntimeState::Stopped);
    CHECK(!seen("start"));
  }
  const auto start_failure = run(ServiceRuntimeState::Running, "start");
  CHECK(!start_failure.ok && start_failure.error.find(L"fixture start rejected") != std::wstring::npos);
  CHECK(fixture::state == ServiceRuntimeState::Stopped);
  CHECK(!run(ServiceRuntimeState::Running, "stop").ok);
  CHECK(!seen("process-stop") && !seen("files") && !seen("start"));
  for (auto initial : {ServiceRuntimeState::Error, ServiceRuntimeState::Other}) {
    CHECK(!run(initial).ok);
    CHECK(fixture::calls == std::vector<std::string>({"query"}));
  }
  std::cout << "PASS: RunInstall preserves Running/Stopped/NotInstalled and stops recovery on failed maintenance, extraction, finalization or start\n";
}
