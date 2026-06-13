int current_process_id() {
#ifdef _WIN32
  return static_cast<int>(GetCurrentProcessId());
#else
  return static_cast<int>(getpid());
#endif
}

std::string registry_path(const std::string &config_dir) {
  return (config_path(config_dir) / kRegistryFileName).string();
}

nlohmann::json to_json(const AttemptRecord &record) {
  return nlohmann::json{{"attempt_id", record.attempt_id},
                        {"owner_pid", record.owner_pid},
                        {"helper_pid", record.helper_pid},
                        {"mode", record.mode},
                        {"created_at_unix_ms", record.created_at_unix_ms},
                        {"state", record.state},
                        {"terminal_reason", record.terminal_reason}};
}

TerminalAttemptScope::TerminalAttemptScope(std::string config_dir,
                                           std::string attempt_id,
                                           std::string reason)
    : config_dir_(std::move(config_dir)), attempt_id_(std::move(attempt_id)),
      reason_(std::move(reason)) {}

TerminalAttemptScope::~TerminalAttemptScope() {
  if (!dismissed_ && !config_dir_.empty() && !attempt_id_.empty()) {
    mark_terminal_if_current(config_dir_, attempt_id_,
                             reason_.empty() ? "scope_exit" : reason_);
  }
}

TerminalAttemptScope::TerminalAttemptScope(
    TerminalAttemptScope &&other) noexcept
    : config_dir_(std::move(other.config_dir_)),
      attempt_id_(std::move(other.attempt_id_)),
      reason_(std::move(other.reason_)), dismissed_(other.dismissed_) {
  other.dismissed_ = true;
}

TerminalAttemptScope &TerminalAttemptScope::operator=(
    TerminalAttemptScope &&other) noexcept {
  if (this != &other) {
    if (!dismissed_ && !config_dir_.empty() && !attempt_id_.empty()) {
      mark_terminal_if_current(config_dir_, attempt_id_,
                               reason_.empty() ? "scope_exit" : reason_);
    }
    config_dir_ = std::move(other.config_dir_);
    attempt_id_ = std::move(other.attempt_id_);
    reason_ = std::move(other.reason_);
    dismissed_ = other.dismissed_;
    other.dismissed_ = true;
  }
  return *this;
}

void TerminalAttemptScope::dismiss() noexcept { dismissed_ = true; }

bool read_record(const std::string &config_dir, AttemptRecord *out) {
  return read_json_record_path(fs::u8path(registry_path(config_dir)), out);
}

AcquireResult try_acquire(const AcquireOptions &options) {
  if (options.config_dir.empty()) {
    return lock_failed_result(
        "Connection attempt guard requires a config directory.");
  }

  std::error_code ec;
  fs::create_directories(config_path(options.config_dir), ec);
  if (ec)
    return lock_failed_result("Failed to create connection attempt directory.");

  AttemptMutex mutex = AttemptMutex::acquire(options.config_dir);
  if (!mutex.acquired())
    return active_result(unknown_active_record());

  const bool lock_exists = fs::exists(lock_path(options.config_dir), ec);
  if (lock_exists) {
    AttemptRecord lock_record;
    if (read_lock_record(options.config_dir, &lock_record)) {
      AttemptRecord effective_record = lock_record;
      AttemptRecord registry_record;
      if (read_record(options.config_dir, &registry_record) &&
          registry_record.attempt_id == lock_record.attempt_id) {
        effective_record = merge_attempt_metadata(registry_record, lock_record);
      }

      if (!is_terminal_state(effective_record.state) &&
          !all_owner_candidates_dead(effective_record, options))
        return active_result(effective_record);
      if (!remove_owned_lock(options.config_dir, lock_record.attempt_id))
        return active_result(effective_record);
    } else {
      AttemptRecord registry_record;
      if (read_record(options.config_dir, &registry_record) &&
          !is_terminal_state(registry_record.state)) {
        if (!all_owner_candidates_dead(registry_record, options))
          return active_result(registry_record);
      }
      remove_lock_for_recovery(options.config_dir);
    }
  }

  AttemptRecord current;
  if (read_record(options.config_dir, &current) &&
      !is_terminal_state(current.state)) {
    if (!all_owner_candidates_dead(current, options))
      return active_result(current);
  }

  return acquired_result(options.config_dir, options);
}

bool update_pids_if_current(const std::string &config_dir,
                            const std::string &attempt_id, int helper_pid) {
  AttemptMutex mutex = AttemptMutex::acquire(config_dir);
  if (!mutex.acquired())
    return false;

  AttemptRecord record;
  if (!read_record(config_dir, &record) || record.attempt_id != attempt_id)
    return false;

  if (helper_pid > 0)
    record.helper_pid = helper_pid;
  if (!write_record(config_dir, record))
    return false;

  AttemptRecord lock_record;
  if (read_lock_record(config_dir, &lock_record) &&
      lock_record.attempt_id == attempt_id) {
    write_lock_owner(lock_path(config_dir),
                     merge_attempt_metadata(record, lock_record));
  }
  return true;
}

bool mark_terminal_if_current(const std::string &config_dir,
                              const std::string &attempt_id,
                              const std::string &reason) {
  AttemptMutex mutex = AttemptMutex::acquire(config_dir);
  if (!mutex.acquired())
    return false;

  AttemptRecord record;
  if (!read_record(config_dir, &record) || record.attempt_id != attempt_id)
    return false;

  record.state = "terminal";
  record.terminal_reason = reason;
  const bool wrote = write_record(config_dir, record);

  remove_owned_lock(config_dir, attempt_id);
  return wrote;
}

bool mark_terminal(const std::string &config_dir, const std::string &reason) {
  AttemptMutex mutex = AttemptMutex::acquire(config_dir);
  if (!mutex.acquired())
    return false;

  AttemptRecord record;
  if (read_record(config_dir, &record)) {
    record.state = "terminal";
    record.terminal_reason = reason;
    write_record(config_dir, record);
  }

  if (!record.attempt_id.empty())
    return remove_owned_lock(config_dir, record.attempt_id);
  return remove_lock_for_recovery(config_dir);
}

