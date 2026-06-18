export module exv.config.contract;

export namespace exv::config::contract {

inline constexpr const char *ACTIONS[] = {
    "config.getAuth",
    "config.saveAuth",
    "config.getSettings",
    "config.saveSettings",
    "config.profile.get",
    "config.profile.save",
};

struct Alias {
  const char *alias;
  const char *target;
};

inline constexpr Alias ALIASES[] = {
    {"config.get", "config.getSettings"},
    {"config.save", "config.saveSettings"},
    {"config.get_profile", "config.profile.get"},
    {"config.save_profile", "config.profile.save"},
};

constexpr bool string_equal(const char *left, const char *right) noexcept {
  if (left == nullptr || right == nullptr) {
    return left == right;
  }
  while (*left != '\0' && *right != '\0') {
    if (*left != *right) {
      return false;
    }
    ++left;
    ++right;
  }
  return *left == *right;
}

constexpr unsigned action_count() noexcept {
  return sizeof(ACTIONS) / sizeof(ACTIONS[0]);
}

constexpr unsigned alias_count() noexcept {
  return sizeof(ALIASES) / sizeof(ALIASES[0]);
}

constexpr bool is_action(const char *action) noexcept {
  for (const auto candidate : ACTIONS) {
    if (string_equal(candidate, action)) {
      return true;
    }
  }
  return false;
}

constexpr bool is_alias(const char *alias) noexcept {
  for (const auto candidate : ALIASES) {
    if (string_equal(candidate.alias, alias)) {
      return true;
    }
  }
  return false;
}

constexpr const char *canonical_action(const char *alias) noexcept {
  for (const auto candidate : ALIASES) {
    if (string_equal(candidate.alias, alias)) {
      return candidate.target;
    }
  }
  return nullptr;
}

} // namespace exv::config::contract
