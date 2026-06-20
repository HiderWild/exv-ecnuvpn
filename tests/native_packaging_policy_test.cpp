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

#ifndef EXV_SOURCE_DIR
#define EXV_SOURCE_DIR "."
#endif

const fs::path kRepoRoot = fs::path(EXV_SOURCE_DIR);

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
  return lower.find("legacy diagnostic") != std::string::npos ||
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
      "scripts/package_ui_shell.py",
      "scripts/embed_assets.py",
      "scripts/build-windows.ps1",
      "scripts/build-macos.sh",
      "docs/runtime-assets.md",
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

bool check_webview_package_policy() {
  bool ok = true;
  const FileText package_script = read_file("scripts/package_ui_shell.py");
  const FileText layout = read_file("webui/scripts/build-layout.cjs");
  const FileText embed = read_file("scripts/embed_assets.py");

  ok = expect(contains(package_script, "assert_no_electron_payload"),
              "package_ui_shell.py should reject bundled Electron/Chromium "
              "payloads") &&
       ok;
  ok = expect(contains(package_script, "WebView2Loader.dll"),
              "package_ui_shell.py should package the WebView2 loader on "
              "Windows") &&
       ok;
  ok = expect(contains(package_script, "MINGW_RUNTIME_DLLS") &&
                  contains(package_script, "copy_windows_runtime_assets"),
              "package_ui_shell.py should package required MinGW runtime DLLs "
              "after Electron staging is removed") &&
       ok;
  ok = expect(contains(package_script, "wintun.dll"),
              "package_ui_shell.py should preserve Wintun as an explicit "
              "Windows runtime asset") &&
       ok;
  ok = expect(contains(package_script, "exv-ui.args"),
              "package_ui_shell.py should write launch arguments for exv-ui") &&
       ok;
  ok = expect(contains(package_script, "validate_launch_args_targets"),
              "package_ui_shell.py should verify packaged launch argument "
              "targets") &&
       ok;
  ok = expect(contains(package_script, "PACKAGE_BINARIES") &&
                  contains(package_script, "\"exv-helper\"") &&
                  contains(package_script, "validate_required_package_binaries"),
              "package_ui_shell.py should require exv-helper in the packaged "
              "WebView shell") &&
       ok;
  ok = expect(!contains(package_script, "--state-dir"),
              "package_ui_shell.py should keep exv-ui.args portable by not "
              "embedding profile paths") &&
       ok;
  ok = expect(contains(package_script, "APP_ICON_ASSETS") &&
                  contains(package_script, "verify_app_icon_assets"),
              "package_ui_shell.py should fail packaging when shared app "
              "icon assets are missing") &&
       ok;
  ok = expect(!contains(layout, "electronRoot") &&
                  !contains(layout, "dist-electron") &&
                  !contains(layout, "nativeBinDir"),
              "build-layout.cjs should expose only WebView package layout "
              "fields") &&
       ok;
  ok = expect(contains(embed, "webview") && !contains(embed, "electron"),
              "embed_assets.py should default to WebView renderer assets") &&
       ok;

  return ok;
}

std::vector<fs::path> active_naming_policy_roots() {
  return {
      "src",
      "scripts",
      "contracts",
      "webui/src",
      "webui/host",
      "README.md",
      "docs/build_guide.md",
      "docs/user_guide.md",
      "docs/CREDENTIAL_STORE_AND_SECRET_MIGRATION.md",
  };
}

bool should_scan_text_file(const fs::path &path) {
  const std::string extension = to_lower(path.extension().generic_string());
  return extension == ".cpp" || extension == ".hpp" || extension == ".h" ||
         extension == ".mm" || extension == ".m" || extension == ".rc" ||
         extension == ".py" || extension == ".ps1" || extension == ".sh" ||
         extension == ".ts" || extension == ".tsx" || extension == ".vue" ||
         extension == ".svg" || extension == ".json" || extension == ".md" ||
         extension == ".cjs";
}

std::vector<FileText> active_naming_policy_files() {
  std::vector<FileText> files;
  for (const fs::path &root : active_naming_policy_roots()) {
    const fs::path full_root = kRepoRoot / root;
    if (!fs::exists(full_root))
      continue;
    if (fs::is_regular_file(full_root)) {
      if (should_scan_text_file(full_root))
        files.push_back(read_file(root));
      continue;
    }

    for (const fs::directory_entry &entry :
         fs::recursive_directory_iterator(full_root,
                                          fs::directory_options::skip_permission_denied)) {
      if (!entry.is_regular_file() || !should_scan_text_file(entry.path()))
        continue;
      const fs::path relative_path = fs::relative(entry.path(), kRepoRoot);
      const std::string normalized = normalize_path_separators(generic_path(relative_path));
      if (normalized.find("/__tests__/") != std::string::npos ||
          normalized.find("/__snapshots__/") != std::string::npos)
        continue;
      files.push_back(read_file(relative_path));
    }
  }
  return files;
}

bool check_exv_naming_policy() {
  bool ok = true;
  const std::string old_dash_name = std::string("ECNU") + "-VPN";
  const std::string old_space_name = std::string("ECNU") + " VPN";
  const std::string old_dist_name = std::string("EXV") + " for " + "ECNU";
  const std::string old_lower_joined = std::string("ecnu") + "vpn";
  const std::string old_lower_dashed = std::string("ecnu") + "-vpn";
  const std::string old_macro_prefix = std::string("ECNU") + "VPN_";
  const std::string old_launchd_label =
      std::string("com.") + "ecnu" + ".exv.helper";
  const std::vector<std::string> denied_needles = {
      old_dash_name,
      old_space_name,
      old_dist_name,
      old_lower_joined,
      old_lower_dashed,
      old_macro_prefix,
      old_launchd_label,
  };
  std::vector<Offense> offenses;

  for (const FileText &file : active_naming_policy_files()) {
    for (size_t i = 0; i < file.lines.size(); ++i) {
      for (const std::string &needle : denied_needles) {
        if (!contains_text(file.lines[i], needle))
          continue;
        const std::string normalized_path =
            normalize_path_separators(generic_path(file.relative_path));
        const bool allowed_repository_metadata =
            (normalized_path == "src/generated/distribution_config.hpp" &&
             (contains_text(file.lines[i], "kRepositoryLabel") ||
              contains_text(file.lines[i], "kRepositoryUrl"))) ||
            (normalized_path == "webui/src/generated/distribution.ts" &&
             (contains_text(file.lines[i], "label:") ||
              contains_text(file.lines[i], "url:")));
        if (allowed_repository_metadata)
          continue;
        offenses.push_back(
            Offense{file.relative_path, needle, static_cast<int>(i + 1),
                    trim(file.lines[i])});
      }
    }
  }

  ok = emit_offenses(offenses,
                     "Active production files contain old generated product "
                     "naming; use EXV/exv instead:") &&
       ok;

  const FileText contract = read_file("contracts/system.contract.json");
  const FileText generator = read_file("scripts/generate_contracts.py");
  ok = expect(contains(contract, "\"contract_id\": \"exv.system\""),
              "system contract id should be exv.system") &&
       ok;
  ok = expect(contains(generator, "exv.system") &&
                  !contains(generator, std::string("ecnu") + "-vpn.system"),
              "contract generator should validate exv.system") &&
       ok;

  return ok;
}

bool check_common_runtime_path_policy() {
  bool ok = true;
  const FileText win_paths = read_file("src/platform/win32/path_utils.cpp");
  const FileText darwin_paths = read_file("src/platform/darwin/path_utils.cpp");
  const FileText win_defaults =
      read_file("src/platform/win32/config_defaults.cpp");
  const FileText darwin_defaults =
      read_file("src/platform/darwin/config_defaults.cpp");
  const FileText win_helper =
      read_file("src/platform/win32/helper_platform.cpp");
  const FileText darwin_helper =
      read_file("src/platform/darwin/helper_platform.cpp");
  const FileText win_service =
      read_file("src/platform/win32/helper_service_manager.cpp");
  const FileText darwin_service =
      read_file("src/platform/darwin/helper_service_manager.cpp");
  const FileText darwin_status =
      read_file("src/platform/darwin/service_status.cpp");
  const FileText win_smoke = read_file("scripts/windows-packaging-smoke.ps1");

  ok = expect(contains(win_paths, "LOCALAPPDATA") &&
                  contains(win_paths, "EXV") &&
                  contains(win_paths, "profile") &&
                  contains(win_paths, "default"),
              "Windows default profile path should resolve under "
              "%LOCALAPPDATA%\\EXV\\profile\\default") &&
       ok;
  ok = expect(contains(darwin_paths, "Library") &&
                  contains(darwin_paths, "Application Support") &&
                  contains(darwin_paths, "EXV") &&
                  contains(darwin_paths, "profile") &&
                  contains(darwin_paths, "default"),
              "macOS default profile path should resolve under "
              "~/Library/Application Support/EXV/profile/default") &&
       ok;
  ok = expect(!contains(win_paths, "\"exv\"") &&
                  !contains(darwin_paths, "\"~/.exv\""),
              "Windows/macOS platform path defaults should not use bare "
              "profile roots") &&
       ok;
  ok = expect(contains(win_defaults, "LOCALAPPDATA") &&
                  contains(win_defaults, "EXV") &&
                  contains(win_defaults, "profile") &&
                  contains(win_defaults, "default") &&
                  contains(win_defaults, "exv.log"),
              "Windows config defaults should put the default log file under "
              "the shared profile directory") &&
       ok;
  ok = expect(contains(darwin_defaults, "Library/Application Support/EXV/"
                                      "profile/default/exv.log"),
              "macOS config defaults should put the default log file under "
              "the shared profile directory") &&
       ok;

  ok = expect(contains(win_helper, "LOCALAPPDATA") &&
                  contains(win_helper, "EXV") &&
                  contains(win_helper, "Helper") &&
                  contains(win_helper, "exv-helper.exe"),
              "Windows helper service binary should have a stable user-local "
              "install path") &&
       ok;
  ok = expect(!contains(win_helper, "Program Files\\\\EXV\\\\exv-helper.exe"),
              "Windows helper service binary must not default to a package or "
              "Program Files path") &&
       ok;
  ok = expect(contains(darwin_helper,
                       "/Library/Application Support/EXV/Helper/"
                       "exv-helper"),
              "macOS LaunchDaemon helper should have a stable system "
              "Application Support path") &&
       ok;
  ok = expect(contains(win_service, "EXV Helper"),
              "Windows helper service display name should be EXV Helper") &&
       ok;
  ok = expect(contains(darwin_helper, "com.exv.helper") &&
                  !contains(darwin_helper,
                            std::string("com.") + "ecnu" + ".exv.helper"),
              "macOS helper service should use the com.exv.helper launchd label") &&
       ok;

  ok = expect(contains(win_service, "default_service_binary_path") &&
                  contains(win_service, "create_directories") &&
                  contains(win_service, "copy_file") &&
                  contains(win_service, "SERVICE_AUTO_START"),
              "Windows service install should copy the current package helper "
              "to the stable helper path before SCM registration") &&
       ok;
  ok = expect(contains(darwin_service, "default_service_binary_path") &&
                  contains(darwin_service, "create_directories") &&
                  contains(darwin_service, "copy_file"),
              "macOS service install should copy the current package helper "
              "to the stable LaunchDaemon helper path") &&
       ok;
  ok = expect(!contains(darwin_status, "/usr/local/bin/exv-helper --service"),
              "macOS service status warnings should describe the configured "
              "stable helper path, not a legacy hard-coded path") &&
       ok;
  ok = expect(contains(win_smoke, "LOCALAPPDATA") &&
                  contains(win_smoke, "EXV") &&
                  contains(win_smoke, "Helper") &&
                  contains(win_smoke, "exv-helper.exe"),
              "Windows packaging smoke should expect SCM to point at the "
              "stable helper copy, not the movable package root") &&
       ok;

  return ok;
}

bool check_webview_shell_icon_assets() {
  bool ok = true;
  const std::vector<fs::path> icon_assets = {
      "assets/icons/icon.ico",
      "assets/icons/icon.icns",
      "assets/icons/icon.png",
      "assets/icons/icon.svg",
  };

  for (const fs::path &relative_path : icon_assets) {
    ok = expect(fs::is_regular_file(kRepoRoot / relative_path),
                "native WebView shell icon asset should exist: " +
                    generic_path(relative_path)) &&
         ok;
  }

  const FileText cmake = read_file("CMakeLists.txt");
  ok = expect(contains(cmake, "${CMAKE_SOURCE_DIR}/assets/icons"),
              "CMake should expose the shared app icon assets to the Windows "
              "resource compiler") &&
       ok;

  const FileText rc =
      read_file("src/platform/win32/ui_shell/exv_ui_win32.rc");
  ok = expect(contains(rc, "IDI_EXV_APP ICON \"icon.ico\""),
              "Windows resource script should embed the shared EXV app "
              "icon") &&
       ok;

  return ok;
}

bool check_retired_electron_artifacts() {
  bool ok = true;
  const std::vector<fs::path> retired_paths = {
      "webui/electron-builder.config.cjs",
      "webui/scripts/build-electron.cjs",
      "webui/scripts/prepare-native.cjs",
      "webui/scripts/run-electron-test.cjs",
      "webui/tsconfig.electron.json",
      "webui/desktop/main",
      "webui/desktop/preload",
      "webui/build-resources",
  };

  for (const fs::path &relative_path : retired_paths) {
    ok = expect(!fs::exists(kRepoRoot / relative_path),
                generic_path(relative_path) +
                    " should not exist in the production WebView UI path") &&
         ok;
  }

  return ok;
}

bool check_legacy_staging_scripts_removed() {
  bool ok = true;
  const std::vector<fs::path> removed_scripts = {
      "scripts/stage-openconnect-runtime-win.ps1",
      "scripts/stage-openconnect-runtime-mac.sh",
  };

  for (const fs::path &script : removed_scripts) {
    ok = expect(!fs::exists(kRepoRoot / script),
                generic_path(script) +
                    " should not exist; legacy runtime staging is not a "
                    "repository script") &&
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
    ok = expect(!contains_ci(script.text, "desktop:package") &&
                    !contains_ci(script.text, "build:electron") &&
                    !contains_ci(script.text, "electron\\release") &&
                    !contains_ci(script.text, "electron/release"),
                name +
                    " should not call retired Electron package scripts or "
                    "release paths") &&
         ok;
  }

  return ok;
}

bool check_runtime_assets_doc_policy() {
  bool ok = true;
  const FileText doc = read_file("docs/runtime-assets.md");

  ok = expect(contains(doc, "Runtime Assets"),
              "docs/runtime-assets.md should document runtime asset policy") &&
       ok;
  ok = expect(contains(doc, "runtime/"),
              "docs/runtime-assets.md should document the ignored runtime "
              "directory") &&
       ok;
  ok = expect(contains(doc, "wintun.dll"),
              "docs/runtime-assets.md should document the Wintun allowlist "
              "asset") &&
       ok;
  ok = expect(!contains_ci(doc.text, "openconnect"),
              "docs/runtime-assets.md should not keep OpenConnect runtime "
              "staging instructions") &&
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
  ok = expect(contains(cmake, "EXV_SOURCE_DIR") &&
                  contains(cmake, "EXV_VERSION") &&
                  contains(cmake, "EXV_PLATFORM_WINDOWS") &&
                  !contains(cmake, std::string("ECNU") + "VPN_SOURCE_DIR") &&
                  !contains(cmake, std::string("ECNU") + "VPN_VERSION") &&
                  !contains(cmake,
                            std::string("ECNU") + "VPN_PLATFORM_WINDOWS"),
              "CMake compile definitions should use the EXV_* namespace") &&
       ok;

  return ok;
}

bool check_active_cmake_legacy_sources_absent() {
  bool ok = true;
  const FileText cmake = read_file("CMakeLists.txt");
  const std::vector<std::string> denied_sources = {
      "openconnect_process",
      "openconnect_log",
      "vpn_legacy_adapter",
      "openconnect_tunnel_script",
  };

  for (const std::string &source : denied_sources) {
    ok = expect(!contains(cmake, source),
                "CMakeLists.txt should not keep active legacy OpenConnect "
                "source/test target references") &&
         ok;
  }

  return ok;
}

bool check_validation_scripts_do_not_name_legacy_payloads() {
  bool ok = true;
  const std::vector<fs::path> scripts = {
      "scripts/validate-merge-prep-windows.ps1",
      "scripts/validate-merge-prep-macos.sh",
      "scripts/macos-packaging-smoke.sh",
  };
  const std::vector<std::string> denied_payload_names = {
      "openconnect",
      "openconnect.exe",
      "libopenconnect",
      "vpnc-script",
      "stage-openconnect-runtime",
  };

  for (const fs::path &script_path : scripts) {
    const FileText script = read_file(script_path);
    for (const std::string &payload : denied_payload_names) {
      ok = expect(!contains_ci(script.text, payload),
                  generic_path(script_path) +
                      " should not hard-code legacy OpenConnect runtime "
                      "payload names") &&
           ok;
    }
  }

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
      "scripts/package_ui_shell.py",
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
      "scripts/package_ui_shell.py",
      "filter: [\n"
      "  '!openconnect.exe',\n"
      "  '!libopenconnect-*',\n"
      "  '!*gnutls*',\n"
      "]\n");
  ok = expect(scan_production_text(deny_filter).empty(),
              "production text scanner should allow explicit deny filters") &&
       ok;

  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = check_scanner_examples() && ok;
  ok = check_production_files_exist_and_scan_cleanly() && ok;
  ok = check_webview_package_policy() && ok;
  ok = check_common_runtime_path_policy() && ok;
  ok = check_webview_shell_icon_assets() && ok;
  ok = check_retired_electron_artifacts() && ok;
  ok = check_legacy_staging_scripts_removed() && ok;
  ok = check_production_build_scripts() && ok;
  ok = check_runtime_assets_doc_policy() && ok;
  ok = check_cmake_wiring() && ok;
  ok = check_exv_naming_policy() && ok;
  ok = check_active_cmake_legacy_sources_absent() && ok;
  ok = check_validation_scripts_do_not_name_legacy_payloads() && ok;
  ok = check_production_runtime_dirs() && ok;

  return ok ? 0 : 1;
}
