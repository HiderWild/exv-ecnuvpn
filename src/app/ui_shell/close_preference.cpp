#include "app/ui_shell/close_preference.hpp"

#include <nlohmann/json.hpp>

#include <fstream>

namespace ecnuvpn::ui_shell {
namespace {

bool is_persistable_close_action(const std::string &action) {
  return action == "tray" || action == "quit";
}

ClosePromptResolution normalize_close_result(const nlohmann::json &result) {
  ClosePromptResolution resolution;

  if (result.is_string()) {
    const std::string action = result.get<std::string>();
    if (action == "cancel" || is_persistable_close_action(action)) {
      resolution.action = action;
    }
    return resolution;
  }

  if (!result.is_object()) {
    return resolution;
  }

  if (result.contains("action") && result["action"].is_string()) {
    const std::string action = result["action"].get<std::string>();
    if (action == "cancel" || is_persistable_close_action(action)) {
      resolution.action = action;
    }
  }
  resolution.remember = result.value("remember", false);
  return resolution;
}

} // namespace

ClosePromptResolution parse_close_prompt_resolution(
    const std::string &request_json) {
  try {
    const auto parsed = nlohmann::json::parse(request_json);
    if (!parsed.is_object() || !parsed.contains("payload") ||
        !parsed["payload"].is_object()) {
      return {};
    }
    const auto &payload = parsed["payload"];
    if (!payload.contains("result")) {
      return {};
    }
    return normalize_close_result(payload["result"]);
  } catch (const nlohmann::json::exception &) {
    return {};
  }
}

std::filesystem::path close_preference_path(
    const std::filesystem::path &state_dir) {
  if (state_dir.empty()) {
    return {};
  }
  return state_dir / "close-preference.json";
}

std::optional<std::string> read_close_preference(
    const std::filesystem::path &state_dir) {
  const auto path = close_preference_path(state_dir);
  if (path.empty()) {
    return std::nullopt;
  }

  std::ifstream input(path);
  if (!input.is_open()) {
    return std::nullopt;
  }

  try {
    const auto parsed = nlohmann::json::parse(input);
    if (!parsed.is_object() || !parsed.contains("action") ||
        !parsed["action"].is_string()) {
      return std::nullopt;
    }
    const std::string action = parsed["action"].get<std::string>();
    if (!is_persistable_close_action(action)) {
      return std::nullopt;
    }
    return action;
  } catch (const nlohmann::json::exception &) {
    return std::nullopt;
  }
}

bool write_close_preference(const std::filesystem::path &state_dir,
                            const std::string &action) {
  if (!is_persistable_close_action(action)) {
    return false;
  }

  const auto path = close_preference_path(state_dir);
  if (path.empty()) {
    return false;
  }

  std::error_code error;
  std::filesystem::create_directories(path.parent_path(), error);
  if (error) {
    return false;
  }

  nlohmann::ordered_json out;
  out["action"] = action;
  std::ofstream output(path, std::ios::trunc);
  if (!output.is_open()) {
    return false;
  }
  output << out.dump(2);
  return output.good();
}

} // namespace ecnuvpn::ui_shell
