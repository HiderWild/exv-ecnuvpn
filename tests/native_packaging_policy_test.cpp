#include <algorithm>
#include <cctype>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

namespace {

namespace fs = std::filesystem;

#ifndef ECNUVPN_SOURCE_DIR
#define ECNUVPN_SOURCE_DIR "."
#endif

const fs::path kRepoRoot = fs::path(ECNUVPN_SOURCE_DIR);
constexpr const char *kLegacyEnv = "ECNUVPN_LEGACY_OPENCONNECT_RUNTIME";

struct FileText {
  fs::path relative_path;
  std::string text;
  std::vector<std::string> lines;
};

struct Offense {
  fs::path relative_path;
  std::string pattern;
  int line = 0;
  std::string excerpt;
};

struct TextPattern {
  std::string label;
  std::string needle;
  bool bare_openconnect = false;
  bool openconnect_dylib = false;
};

bool expect(bool condition, const std::string &message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

std::string generic_path(const fs::path &path) {
  return path.generic_string();
}

std::string to_lower(std::string value) {
  std::transform(value.begin(), value.end(), value.begin(), [](unsigned char c) {
    return static_cast<char>(std::tolower(c));
  });
  return value;
}

std::string trim(const std::string &value) {
  const auto first = value.find_first_not_of(" \t\r\n");
  if (first == std::string::npos)
    return {};
  const auto last = value.find_last_not_of(" \t\r\n");
  return value.substr(first, last - first + 1);
}

bool contains_ci(const std::string &haystack, const std::string &needle) {
  return to_lower(haystack).find(to_lower(needle)) != std::string::npos;
}

std::string normalize_path_separators(std::string value) {
  std::replace(value.begin(), value.end(), '\\', '/');
  return value;
}

bool starts_with(const std::string &value, const std::string &prefix) {
  return value.size() >= prefix.size() &&
         value.compare(0, prefix.size(), prefix) == 0;
}

bool ends_with(const std::string &value, const std::string &suffix) {
  return value.size() >= suffix.size() &&
         value.compare(value.size() - suffix.size(), suffix.size(), suffix) == 0;
}

std::vector<std::string> split_lines(const std::string &text) {
  std::vector<std::string> lines;
  std::istringstream stream(text);
  std::string line;
  while (std::getline(stream, line)) {
    if (!line.empty() && line.back() == '\r')
      line.pop_back();
    lines.push_back(line);
  }
  if (text.empty())
    lines.push_back("");
  return lines;
}

std::string slurp(const fs::path &relative_path) {
  const fs::path full_path = kRepoRoot / relative_path;
  std::ifstream input(full_path, std::ios::binary);
  if (!input) {
    std::cerr << "Unable to read " << full_path << std::endl;
    return {};
  }

  return std::string(std::istreambuf_iterator<char>(input),
                     std::istreambuf_iterator<char>());
}

FileText read_file(const fs::path &relative_path) {
  FileText file{relative_path, slurp(relative_path), {}};
  file.lines = split_lines(file.text);
  return file;
}

FileText make_text_file(const fs::path &relative_path, const std::string &text) {
  return FileText{relative_path, text, split_lines(text)};
}

bool contains(const FileText &file, const std::string &needle) {
  return file.text.find(needle) != std::string::npos;
}

bool contains_text(const std::string &text, const std::string &needle) {
  return text.find(needle) != std::string::npos;
}

std::string context_for_line(const FileText &file, size_t index,
                             size_t radius = 8) {
  if (file.lines.empty())
    return {};

  const size_t first = index > radius ? index - radius : 0;
  const size_t last = std::min(file.lines.size() - 1, index + radius);
  std::ostringstream out;
  for (size_t i = first; i <= last; ++i) {
    out << file.lines[i] << '\n';
  }
  return out.str();
}

bool is_token_boundary(char c) {
  const auto value = static_cast<unsigned char>(c);
  return !std::isalnum(value) && c != '_' && c != '-';
}

bool contains_bare_openconnect(const std::string &line) {
  const std::string lower = to_lower(line);
  const std::string needle = "openconnect";
  size_t pos = lower.find(needle);
  while (pos != std::string::npos) {
    const bool left_ok = pos == 0 || is_token_boundary(lower[pos - 1]);
    const size_t after = pos + needle.size();
    const bool right_ok = after >= lower.size() || is_token_boundary(lower[after]);
    if (left_ok && right_ok)
      return true;
    pos = lower.find(needle, pos + 1);
  }
  return false;
}

bool is_builder_deny_filter_line(const std::string &line) {
  const std::string stripped = trim(line);
  return starts_with(stripped, "'!") || starts_with(stripped, "\"!") ||
         starts_with(stripped, "`!");
}

bool is_policy_denial_context(const std::string &context) {
  const std::string lower = to_lower(context);
  const std::vector<std::string> phrases = {
      "must not",       "does not",      "do not",     "not require",
      "not bundle",    "not copy",      "deny",       "denies",
      "denied",        "denylist",      "denying",    "filter should deny",
      "skipping",
  };
  for (const std::string &phrase : phrases) {
    if (lower.find(phrase) != std::string::npos)
      return true;
  }
  return false;
}

bool is_clear_legacy_context(const std::string &context) {
  const std::string lower = to_lower(context);
  return lower.find(to_lower(kLegacyEnv)) != std::string::npos ||
         lower.find("legacy diagnostic") != std::string::npos ||
         lower.find("legacy-only") != std::string::npos ||
         lower.find("legacy openconnect") != std::string::npos ||
         lower.find("legacyopenconnectruntime") != std::string::npos ||
         lower.find("copylegacyruntimeassets") != std::string::npos;
}

bool is_allowed_production_reference(const FileText &file, const std::string &line,
                                     const std::string &context) {
  const std::string lower_line = to_lower(line);
  const std::string lower_context = to_lower(context);

  if (is_builder_deny_filter_line(lower_line))
    return true;
  if (is_policy_denial_context(lower_context))
    return true;
  if (is_clear_legacy_context(lower_context))
    return true;

  const std::string path = generic_path(file.relative_path);
  if (path == "runtime/README.md" &&
      lower_context.find("diagnostic") != std::string::npos) {
    return true;
  }

  return false;
}

bool has_payload_context_for_bare_openconnect(const std::string &line,
                                              const std::string &context) {
  if (!contains_bare_openconnect(line))
    return false;

  const std::string lower = to_lower(line + "\n" + context);
  const std::vector<std::string> payload_terms = {
      "stage",       "staged",      "staging",       "bundle",
      "bundled",     "copy",        "copied",        "runtime",
      "binary",      "payload",     ".dylib",        ".dll",
      "extraresources",             "path.join",     "'openconnect'",
      "\"openconnect\"",            "`openconnect`", "/openconnect",
      "\\openconnect",              "openconnect-path",
  };
  for (const std::string &term : payload_terms) {
    if (lower.find(term) != std::string::npos)
      return true;
  }
  return false;
}

std::vector<TextPattern> denied_text_patterns() {
  return {
      {"stage-openconnect-runtime", "stage-openconnect-runtime"},
      {"openconnect.exe", "openconnect.exe"},
      {"bare binary openconnect", "", true},
      {"libopenconnect", "libopenconnect"},
      {"gnutls/libgnutls", "gnutls"},
      {"libhogweed", "libhogweed"},
      {"libnettle", "libnettle"},
      {"libp11-kit", "libp11-kit"},
      {"libtasn1", "libtasn1"},
      {"libunistring", "libunistring"},
      {"libidn2", "libidn2"},
      {"libstoken", "libstoken"},
      {"OpenConnect dylib names", "", false, true},
  };
}

bool text_pattern_matches(const TextPattern &pattern, const std::string &line,
                          const std::string &context) {
  const std::string lower_line = to_lower(line);
  if (pattern.bare_openconnect &&
      (lower_line.find("openconnect.exe") != std::string::npos ||
       lower_line.find("libopenconnect") != std::string::npos)) {
    return false;
  }
  if (pattern.bare_openconnect)
    return has_payload_context_for_bare_openconnect(line, context);
  if (pattern.openconnect_dylib)
    return lower_line.find("openconnect") != std::string::npos &&
           lower_line.find(".dylib") != std::string::npos;
  return lower_line.find(to_lower(pattern.needle)) != std::string::npos;
}

std::vector<Offense> scan_production_text(const FileText &file) {
  std::vector<Offense> offenses;
  const auto patterns = denied_text_patterns();

  for (size_t i = 0; i < file.lines.size(); ++i) {
    const std::string context = context_for_line(file, i);
    for (const TextPattern &pattern : patterns) {
      if (!text_pattern_matches(pattern, file.lines[i], context))
        continue;
      if (is_allowed_production_reference(file, file.lines[i], context))
        continue;

      offenses.push_back(Offense{file.relative_path, pattern.label,
                                 static_cast<int>(i + 1), trim(file.lines[i])});
    }
  }

  return offenses;
}

bool emit_offenses(const std::vector<Offense> &offenses,
                   const std::string &heading) {
  if (offenses.empty())
    return true;

  std::cerr << heading << std::endl;
  for (const Offense &offense : offenses) {
    std::cerr << "  " << generic_path(offense.relative_path);
    if (offense.line > 0)
      std::cerr << ":" << offense.line;
    std::cerr << " [" << offense.pattern << "]";
    if (!offense.excerpt.empty())
      std::cerr << " " << offense.excerpt;
    std::cerr << std::endl;
  }
  return false;
}

std::vector<fs::path> production_files() {
  return {
      "webui/package.json",
      "webui/scripts/build-layout.cjs",
      "webui/scripts/build-electron.cjs",
      "webui/scripts/prepare-native.cjs",
      "webui/electron-builder.config.cjs",
      "scripts/build-windows.ps1",
      "scripts/build-macos.sh",
      "runtime/README.md",
  };
}

bool check_production_files_exist_and_scan_cleanly() {
  bool ok = true;
  std::vector<Offense> offenses;

  for (const fs::path &relative_path : production_files()) {
    ok = expect(fs::is_regular_file(kRepoRoot / relative_path),
                "production packaging scan target should exist: " +
                    generic_path(relative_path)) &&
         ok;

    FileText file = read_file(relative_path);
    std::vector<Offense> file_offenses = scan_production_text(file);
    offenses.insert(offenses.end(), file_offenses.begin(), file_offenses.end());
  }

  ok = emit_offenses(
           offenses,
           "Production packaging files contain denied OpenConnect/GnuTLS "
           "payload references:") &&
       ok;
  return ok;
}

bool check_prepare_native_policy() {
  bool ok = true;
  const FileText prepare = read_file("webui/scripts/prepare-native.cjs");

  ok = expect(contains(prepare, kLegacyEnv),
              "prepare-native.cjs should gate legacy runtime copying with " +
                  std::string(kLegacyEnv)) &&
       ok;
  ok = expect(!contains(prepare, "copyRecursive(runtimeSource, outDir)"),
              "prepare-native.cjs must not copy runtime directories wholesale "
              "for production packaging") &&
       ok;
  ok = expect(!contains(prepare, "if (runtimeSource)"),
              "prepare-native.cjs runtime directory copying should be explicit "
              "legacy-only, not the production default") &&
       ok;
  ok = expect(contains(prepare, "ALLOWED_NATIVE_RUNTIME_ASSETS"),
              "prepare-native.cjs should copy production runtime assets from an "
              "explicit allowlist") &&
       ok;
  ok = expect(contains(prepare, "wintun.dll"),
              "prepare-native.cjs should still preserve Wintun validation and "
              "copying") &&
       ok;
  ok = expect(contains(prepare, "MINGW_RUNTIME_DLLS"),
              "prepare-native.cjs should still stage required MinGW runtime "
              "DLLs on Windows") &&
       ok;
  ok = expect(contains(prepare, "exv-helper"),
              "prepare-native.cjs should stage the native helper binary") &&
       ok;

  return ok;
}

bool check_builder_denies_legacy_runtime() {
  bool ok = true;
  const FileText builder = read_file("webui/electron-builder.config.cjs");

  const std::vector<std::string> denied_filters = {
      "!openconnect.exe",     "!openconnect",    "!libopenconnect-*",
      "!libopenconnect*.dylib", "!libgnutls-*",  "!*gnutls*",
  };

  for (const std::string &filter : denied_filters) {
    ok = expect(contains(builder, filter),
                "electron-builder extraResources/bin filter should deny " +
                    filter) &&
         ok;
  }

  return ok;
}

bool check_legacy_staging_script_gates() {
  bool ok = true;
  const std::vector<FileText> scripts = {
      read_file("scripts/stage-openconnect-runtime-win.ps1"),
      read_file("scripts/stage-openconnect-runtime-mac.sh"),
  };

  for (const FileText &script : scripts) {
    const std::string name = generic_path(script.relative_path);
    ok = expect(contains(script, kLegacyEnv),
                name + " should require " + kLegacyEnv + "=1") &&
         ok;
    ok = expect(contains_ci(script.text, "legacy diagnostic"),
                name + " should describe OpenConnect staging as legacy "
                       "diagnostic-only") &&
         ok;

    const std::string normalized_text = normalize_path_separators(script.text);
    ok = expect(contains_text(normalized_text, "runtime/legacy-openconnect"),
                name + " should stage OpenConnect only under "
                       "runtime/legacy-openconnect") &&
         ok;
    ok = expect(!contains_text(normalized_text, "runtime/win32-"),
                name + " must not target production runtime/win32-* roots "
                       "for OpenConnect payload staging") &&
         ok;
    ok = expect(!contains_text(normalized_text, "runtime/darwin-"),
                name + " must not target production runtime/darwin-* roots "
                       "for OpenConnect payload staging") &&
         ok;
  }

  return ok;
}

bool check_production_build_scripts() {
  bool ok = true;
  const std::vector<FileText> scripts = {
      read_file("scripts/build-windows.ps1"),
      read_file("scripts/build-macos.sh"),
  };

  for (const FileText &script : scripts) {
    const std::string name = generic_path(script.relative_path);
    ok = expect(contains(script, "native_packaging_policy_test"),
                name + " should build and run native_packaging_policy_test") &&
         ok;
  }

  return ok;
}

bool check_runtime_readme_policy() {
  bool ok = true;
  const FileText readme = read_file("runtime/README.md");

  ok = expect(contains(readme, "Native Production Runtime"),
              "runtime/README.md should document the native production runtime") &&
       ok;
  ok = expect(contains(readme, kLegacyEnv),
              "runtime/README.md should document the explicit legacy "
              "OpenConnect gate") &&
       ok;
  ok = expect(contains_ci(readme.text, "legacy diagnostic"),
              "runtime/README.md should describe OpenConnect runtime assets as "
              "legacy diagnostic-only") &&
       ok;
  ok = expect(!contains(readme, "Bundled OpenConnect Runtime"),
              "runtime/README.md should not present OpenConnect as the bundled "
              "production runtime") &&
       ok;

  return ok;
}

bool check_cmake_wiring() {
  bool ok = true;
  const FileText cmake = read_file("CMakeLists.txt");

  ok = expect(contains(cmake, "add_executable(native_packaging_policy_test"),
              "CMakeLists.txt should define native_packaging_policy_test") &&
       ok;
  ok = expect(contains(cmake, "add_test(NAME native_packaging_policy_test"),
              "CMakeLists.txt should register native_packaging_policy_test "
              "with CTest") &&
       ok;

  return ok;
}

bool is_legacy_runtime_path(const fs::path &relative_path) {
  fs::path parent = relative_path.parent_path();
  for (const fs::path &part : parent) {
    if (to_lower(part.generic_string()).find("legacy-openconnect") !=
        std::string::npos) {
      return true;
    }
  }
  return false;
}

std::string denied_runtime_filename_pattern(const std::string &filename) {
  const std::string lower = to_lower(filename);

  if (lower == "openconnect.exe")
    return "openconnect.exe";
  if (lower == "openconnect")
    return "bare binary openconnect";
  if (lower.find("stage-openconnect-runtime") != std::string::npos)
    return "stage-openconnect-runtime";
  if (lower.find("libopenconnect") != std::string::npos)
    return "libopenconnect";
  if (lower.find("gnutls") != std::string::npos)
    return "gnutls/libgnutls";
  if (lower.find("libhogweed") != std::string::npos)
    return "libhogweed";
  if (lower.find("libnettle") != std::string::npos)
    return "libnettle";
  if (lower.find("libp11-kit") != std::string::npos)
    return "libp11-kit";
  if (lower.find("libtasn1") != std::string::npos)
    return "libtasn1";
  if (lower.find("libunistring") != std::string::npos)
    return "libunistring";
  if (lower.find("libidn2") != std::string::npos)
    return "libidn2";
  if (lower.find("libstoken") != std::string::npos)
    return "libstoken";
  if (lower.find("openconnect") != std::string::npos &&
      ends_with(lower, ".dylib"))
    return "OpenConnect dylib names";

  return {};
}

bool is_production_runtime_root_name(const std::string &name) {
  const std::string lower = to_lower(name);
  return lower == "win32" || lower == "darwin" || starts_with(lower, "win32-") ||
         starts_with(lower, "darwin-");
}

std::vector<fs::path> production_runtime_roots() {
  std::vector<fs::path> roots;
  const fs::path runtime_dir = kRepoRoot / "runtime";
  if (!fs::is_directory(runtime_dir))
    return roots;

  for (const fs::directory_entry &entry : fs::directory_iterator(runtime_dir)) {
    if (!entry.is_directory())
      continue;
    if (is_production_runtime_root_name(entry.path().filename().generic_string()))
      roots.push_back(entry.path());
  }

  return roots;
}

bool check_production_runtime_dirs() {
  std::vector<Offense> offenses;

  for (const fs::path &root : production_runtime_roots()) {
    for (const fs::directory_entry &entry : fs::recursive_directory_iterator(
             root, fs::directory_options::skip_permission_denied)) {
      const fs::path relative_path = fs::relative(entry.path(), kRepoRoot);
      if (is_legacy_runtime_path(relative_path))
        continue;

      const std::string pattern =
          denied_runtime_filename_pattern(entry.path().filename().generic_string());
      if (pattern.empty())
        continue;

      offenses.push_back(Offense{relative_path, pattern, 0, ""});
    }
  }

  return emit_offenses(
      offenses,
      "Production runtime directories contain denied OpenConnect/GnuTLS "
      "payload filenames:");
}

bool check_scanner_examples() {
  bool ok = true;
  const FileText bad_allowlist = make_text_file(
      "webui/scripts/prepare-native.cjs",
      "const ALLOWED_NATIVE_RUNTIME_ASSETS = new Set([\n"
      "  'wintun.dll',\n"
      "  'openconnect.exe',\n"
      "  'libopenconnect-5.dll',\n"
      "  'libgnutls-30.dll',\n"
      "])\n");

  const std::vector<Offense> bad_offenses =
      scan_production_text(bad_allowlist);
  ok = expect(bad_offenses.size() == 3,
              "production text scanner should catch openconnect.exe, "
              "libopenconnect, and gnutls when added beside wintun.dll") &&
       ok;

  const FileText deny_filter = make_text_file(
      "webui/electron-builder.config.cjs",
      "filter: [\n"
      "  '!openconnect.exe',\n"
      "  '!libopenconnect-*',\n"
      "  '!*gnutls*',\n"
      "]\n");
  ok = expect(scan_production_text(deny_filter).empty(),
              "production text scanner should allow explicit deny filters") &&
       ok;

  const FileText legacy_note = make_text_file(
      "runtime/README.md",
      "## Legacy Diagnostic OpenConnect Runtime\n"
      "Set ECNUVPN_LEGACY_OPENCONNECT_RUNTIME=1 before invoking diagnostics.\n"
      "Those scripts may stage openconnect.exe and libopenconnect for "
      "diagnostics.\n");
  ok = expect(scan_production_text(legacy_note).empty(),
              "production text scanner should allow clear legacy diagnostic "
              "documentation") &&
       ok;

  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = check_scanner_examples() && ok;
  ok = check_production_files_exist_and_scan_cleanly() && ok;
  ok = check_prepare_native_policy() && ok;
  ok = check_builder_denies_legacy_runtime() && ok;
  ok = check_legacy_staging_script_gates() && ok;
  ok = check_production_build_scripts() && ok;
  ok = check_runtime_readme_policy() && ok;
  ok = check_cmake_wiring() && ok;
  ok = check_production_runtime_dirs() && ok;

  return ok ? 0 : 1;
}
