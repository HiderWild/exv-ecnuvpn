#include "app/ui_shell/ui_shell_options.hpp"

#include <filesystem>
#include <fstream>
#include <string>
#include <string_view>
#include <vector>

namespace ecnuvpn::ui_shell {

namespace {

UiShellOptions parse_ui_shell_tokens(const std::vector<std::string> &tokens) {
  std::vector<char *> argv;
  argv.reserve(tokens.size() + 1);
  char program[] = "exv-ui";
  argv.push_back(program);
  for (const std::string &token : tokens) {
    argv.push_back(const_cast<char *>(token.c_str()));
  }
  return parse_ui_shell_options(static_cast<int>(argv.size()), argv.data());
}

std::string resolve_sidecar_path(const std::filesystem::path &package_root,
                                 const std::string &value) {
  const std::filesystem::path path(value);
  if (path.is_absolute()) {
    return path.lexically_normal().string();
  }
  return (package_root / path).lexically_normal().string();
}

bool is_inside_or_equal(const std::filesystem::path &path,
                        const std::filesystem::path &root) {
  const auto normalized_path = path.lexically_normal();
  const auto normalized_root = root.lexically_normal();
  if (normalized_path == normalized_root) {
    return true;
  }
  for (auto parent = normalized_path.parent_path(); !parent.empty();
       parent = parent.parent_path()) {
    if (parent == normalized_root) {
      return true;
    }
    if (parent == parent.parent_path()) {
      break;
    }
  }
  return false;
}

std::string validate_packaged_file(const std::string &value,
                                   const std::filesystem::path &package_root,
                                   std::string_view label) {
  if (value.empty()) {
    return {};
  }
  const std::filesystem::path path(value);
  if (!is_inside_or_equal(path, package_root)) {
    return std::string("packaged ") + std::string(label) +
           " path escapes package root";
  }
  if (!std::filesystem::exists(path)) {
    return std::string("packaged ") + std::string(label) +
           " path does not exist";
  }
  return {};
}

} // namespace

UiShellOptions parse_ui_shell_options(int argc, char **argv) {
  UiShellOptions options;
  for (int i = 1; i < argc; ++i) {
    std::string_view arg = argv[i] ? argv[i] : "";
    if (arg == "--renderer-url" && i + 1 < argc) {
      options.renderer_dev_server_url = argv[++i];
    } else if (arg == "--renderer-index" && i + 1 < argc) {
      options.packaged_renderer_index = argv[++i];
    } else if (arg == "--exv" && i + 1 < argc) {
      options.exv_path = argv[++i];
    } else if (arg == "--devtools") {
      options.enable_dev_tools = true;
    }
  }
  return options;
}

UiShellOptions parse_ui_shell_args_file(
    const std::filesystem::path &args_file) {
  std::ifstream sidecar(args_file);
  if (!sidecar) {
    return {};
  }

  std::vector<std::string> tokens;
  std::string line;
  while (std::getline(sidecar, line)) {
    if (!line.empty() && line.back() == '\r') {
      line.pop_back();
    }
    if (!line.empty()) {
      tokens.push_back(line);
    }
  }

  UiShellOptions options = parse_ui_shell_tokens(tokens);
  const std::filesystem::path package_root = args_file.parent_path();
  if (!options.exv_path.empty()) {
    options.exv_path = resolve_sidecar_path(package_root, options.exv_path);
  }
  if (!options.packaged_renderer_index.empty()) {
    options.packaged_renderer_index =
        resolve_sidecar_path(package_root, options.packaged_renderer_index);
  }
  return options;
}

UiShellOptions load_packaged_ui_shell_options(
    const std::filesystem::path &executable_path) {
  const std::filesystem::path package_root = executable_path.parent_path();
  if (package_root.empty()) {
    return {};
  }
  return parse_ui_shell_args_file(package_root / "exv-ui.args");
}

UiShellOptions resolve_ui_shell_options(
    int argc, char **argv, const std::filesystem::path &executable_path) {
  UiShellOptions options = parse_ui_shell_options(argc, argv);
  if (argc > 1 || validate_ui_shell_options(options).empty()) {
    return options;
  }

  const std::filesystem::path package_root = executable_path.parent_path();
  UiShellOptions packaged_options =
      load_packaged_ui_shell_options(executable_path);
  if (validate_ui_shell_options(packaged_options).empty() &&
      validate_packaged_ui_shell_options(packaged_options, package_root).empty()) {
    return packaged_options;
  }
  return options;
}

std::string validate_ui_shell_options(const UiShellOptions &options) {
  if (options.exv_path.empty()) {
    return "missing required --exv path";
  }
  if (options.renderer_dev_server_url.empty() &&
      options.packaged_renderer_index.empty()) {
    return "missing required renderer URL or index path";
  }
  if (!options.renderer_dev_server_url.empty() &&
      !options.packaged_renderer_index.empty()) {
    return "choose either --renderer-url or --renderer-index, not both";
  }
  return {};
}

std::string validate_packaged_ui_shell_options(
    const UiShellOptions &options, const std::filesystem::path &package_root) {
  std::string error = validate_packaged_file(options.exv_path, package_root,
                                             "--exv");
  if (!error.empty()) {
    return error;
  }
  return validate_packaged_file(options.packaged_renderer_index, package_root,
                                "--renderer-index");
}

} // namespace ecnuvpn::ui_shell
