#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <vector>

namespace fs = std::filesystem;

#ifndef ECNUVPN_SOURCE_DIR
#define ECNUVPN_SOURCE_DIR "."
#endif

namespace {

const fs::path kRepoRoot = fs::path(ECNUVPN_SOURCE_DIR);

bool contains(const std::string &haystack, const std::string &needle) {
  return haystack.find(needle) != std::string::npos;
}

std::string generic_relative_path(const fs::path &path) {
  std::error_code ec;
  const fs::path relative = fs::relative(path, kRepoRoot, ec);
  return (ec ? path : relative).generic_string();
}

bool allowed_path(const fs::path &path) {
  const std::string s = "/" + generic_relative_path(path);
  return contains(s, "/docs/archive/") || contains(s, "/reference/") ||
         contains(s, "/tests/") ||
         contains(s,
                  "/docs/handoffs/native-anyconnect-v2-live-validation-template.md") ||
         (contains(s, "/docs/handoffs/") &&
          contains(s, "native-only-live-validation.md")) ||
         contains(s, "/docs/superpowers/plans/") ||
         contains(s, "/docs/superpowers/checklists/");
}

bool ignored_directory(const fs::path &path) {
  const std::string name = path.filename().generic_string();
  return name == ".git" || name == ".worktrees" || name == "build" ||
         name == "build-windows" || name == "node_modules" ||
         name == "dist" || name == ".vite";
}

bool scanned_extension(const fs::path &path) {
  const std::string ext = path.extension().string();
  return ext == ".cpp" || ext == ".hpp" || ext == ".ts" ||
         ext == ".vue" || ext == ".json" || ext == ".md" ||
         ext == ".yml" || ext == ".yaml" || ext == ".cmake" ||
         path.filename() == "CMakeLists.txt";
}

std::string slurp(const fs::path &path) {
  std::ifstream in(path, std::ios::binary);
  return std::string(std::istreambuf_iterator<char>(in),
                     std::istreambuf_iterator<char>());
}

} // namespace

int main() {
  const std::vector<std::string> banned = {
      "__vpn-supervisor",
      "legacy_openconnect",
      "openconnect_process",
      "spawn_openconnect_process",
      "openconnect_tunnel_script",
      "openconnect_log",
      "configure_from_openconnect_log",
      "webvpn_session=",
  };

  bool ok = true;
  fs::recursive_directory_iterator it(kRepoRoot,
                                      fs::directory_options::skip_permission_denied);
  const fs::recursive_directory_iterator end;
  for (; it != end; ++it) {
    if (it->is_directory() && ignored_directory(it->path())) {
      it.disable_recursion_pending();
      continue;
    }
    if (!it->is_regular_file())
      continue;

    const fs::path path = it->path();
    if (!scanned_extension(path) || allowed_path(path))
      continue;

    const std::string text = slurp(path);
    for (const std::string &needle : banned) {
      if (!contains(text, needle))
        continue;

      std::cerr << "native-only banned token " << needle << " in "
                << generic_relative_path(path) << "\n";
      ok = false;
    }
  }

  return ok ? 0 : 1;
}
