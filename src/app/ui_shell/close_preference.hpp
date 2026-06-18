#pragma once

#include <filesystem>
#include <optional>
#include <string>

namespace ecnuvpn::ui_shell {

struct ClosePromptResolution {
  std::string action = "cancel";
  bool remember = false;
};

ClosePromptResolution parse_close_prompt_resolution(
    const std::string &request_json);

std::filesystem::path close_preference_path(
    const std::filesystem::path &state_dir);

std::optional<std::string> read_close_preference(
    const std::filesystem::path &state_dir);

bool write_close_preference(const std::filesystem::path &state_dir,
                            const std::string &action);

} // namespace ecnuvpn::ui_shell
