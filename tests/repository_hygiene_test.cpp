#include <filesystem>
#include <iostream>
#include <string>
#include <vector>

namespace {

namespace fs = std::filesystem;

#ifndef ECNUVPN_SOURCE_DIR
#define ECNUVPN_SOURCE_DIR "."
#endif

const fs::path kRepoRoot = fs::path(ECNUVPN_SOURCE_DIR);

bool expect(bool condition, const std::string &message) {
  if (condition)
    return true;
  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

std::string lower(std::string value) {
  for (char &ch : value) {
    if (ch >= 'A' && ch <= 'Z')
      ch = static_cast<char>(ch - 'A' + 'a');
  }
  return value;
}

bool has_forbidden_root_artifact_extension(const fs::path &path) {
  const std::string extension = lower(path.extension().generic_string());
  return extension == ".a" || extension == ".lib" || extension == ".dll" ||
         extension == ".dylib" || extension == ".so";
}

bool check_root_markdown_policy() {
  bool ok = true;
  for (const fs::directory_entry &entry : fs::directory_iterator(kRepoRoot)) {
    if (!entry.is_regular_file())
      continue;
    if (lower(entry.path().extension().generic_string()) != ".md")
      continue;

    const std::string name = entry.path().filename().generic_string();
    ok = expect(name == "README.md",
                "root markdown files must be consolidated under docs/ except "
                "for README.md: " +
                    name) &&
         ok;
  }
  return ok;
}

bool check_root_artifact_policy() {
  bool ok = true;
  for (const fs::directory_entry &entry : fs::directory_iterator(kRepoRoot)) {
    if (!entry.is_regular_file())
      continue;
    if (!has_forbidden_root_artifact_extension(entry.path()))
      continue;

    ok = expect(false,
                "build/runtime artifacts must not be placed in the repository "
                "root: " +
                    entry.path().filename().generic_string()) &&
         ok;
  }
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = check_root_markdown_policy() && ok;
  ok = check_root_artifact_policy() && ok;
  return ok ? 0 : 1;
}
