#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

#ifndef ECNUVPN_SOURCE_DIR
#define ECNUVPN_SOURCE_DIR "."
#endif

namespace {

bool expect(bool condition, const std::string &message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

std::string read_file(const std::filesystem::path &path) {
  std::ifstream input(path);
  std::ostringstream output;
  output << input.rdbuf();
  return output.str();
}

bool is_source_file(const std::filesystem::path &path) {
  const auto ext = path.extension().string();
  return ext == ".h" || ext == ".hpp" || ext == ".cpp" ||
         ext == ".cppm" || ext == ".ipp";
}

bool tree_contains_forbidden_include(
    const std::filesystem::path &root,
    const std::vector<std::string> &needles,
    std::vector<std::filesystem::path> &offenders) {
  if (!std::filesystem::exists(root)) {
    return false;
  }

  for (const auto &entry : std::filesystem::recursive_directory_iterator(root)) {
    if (!entry.is_regular_file() || !is_source_file(entry.path())) {
      continue;
    }

    const auto content = read_file(entry.path());
    for (const auto &needle : needles) {
      if (content.find(needle) != std::string::npos) {
        offenders.push_back(entry.path());
        break;
      }
    }
  }

  return !offenders.empty();
}

bool expect_tree_has_no_forbidden_includes(
    const std::filesystem::path &root,
    const std::vector<std::string> &needles,
    const std::string &message) {
  std::vector<std::filesystem::path> offenders;
  const bool found =
      tree_contains_forbidden_include(root, needles, offenders);
  bool ok = expect(!found, message);
  for (const auto &offender : offenders) {
    std::cerr << "  offender: " << offender.string() << '\n';
  }
  return ok;
}

bool expect_no_legacy_logger_include_outside_compat(
    const std::filesystem::path &src_root) {
  bool ok = true;
  const auto allowed = src_root / "common" / "diagnostics" / "logger.cpp";
  for (const auto &entry :
       std::filesystem::recursive_directory_iterator(src_root)) {
    if (!entry.is_regular_file() || !is_source_file(entry.path())) {
      continue;
    }
    if (std::filesystem::equivalent(entry.path(), allowed)) {
      continue;
    }
    const auto content = read_file(entry.path());
    if (content.find("#include \"common/diagnostics/logger.hpp\"") !=
        std::string::npos) {
      ok &= expect(false,
                   "legacy logger include is only allowed in compatibility "
                   "implementation: " +
                       entry.path().string());
    }
  }
  return ok;
}

} // namespace

int main() {
  const auto source_root = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  const auto src_root = source_root / "src";
  const auto base_root = src_root / "base";
  const auto observability_root = src_root / "observability";
  const auto diagnostics_root = src_root / "common" / "diagnostics";

  bool ok = true;

  ok &= expect(std::filesystem::exists(source_root / "docs" / "superpowers" /
                                       "plans" /
                                       "2026-06-15-base-observability-"
                                       "foundation-plan.md"),
               "base observability plan must be public under docs");
  ok &= expect(std::filesystem::exists(source_root / "docs" / "superpowers" /
                                       "specs" /
                                       "2026-06-15-base-observability-"
                                       "foundation-design.md"),
               "base observability design must be public under docs");
  ok &= expect(std::filesystem::exists(diagnostics_root / "logger.hpp"),
               "diagnostics compatibility logger must exist during migration");

  const std::vector<std::string> upward_includes{
      "#include \"platform/", "#include <platform/",
      "#include \"core/",     "#include <core/",
      "#include \"helper/",   "#include <helper/",
      "#include \"vpn_engine/", "#include <vpn_engine/",
      "#include \"runtime/",  "#include <runtime/",
      "#include \"app/",      "#include <app/",
      "#include \"cli/",      "#include <cli/",
  };
  auto base_forbidden_includes = upward_includes;
  base_forbidden_includes.push_back("#include \"observability/");
  base_forbidden_includes.push_back("#include <observability/");

  if (std::filesystem::exists(base_root)) {
    ok &= expect_tree_has_no_forbidden_includes(
        base_root, base_forbidden_includes,
        "src/base must not include upward runtime or platform layers");
  }

  if (std::filesystem::exists(observability_root)) {
    ok &= expect_tree_has_no_forbidden_includes(
        observability_root, upward_includes,
        "src/observability must not include upward runtime or platform layers");
  }

  ok &= expect_no_legacy_logger_include_outside_compat(src_root);

  if (!ok) {
    std::cerr << "base_observability_architecture_test: FAILED\n";
    return 1;
  }

  std::cout << "base_observability_architecture_test: all assertions passed\n";
  return 0;
}
