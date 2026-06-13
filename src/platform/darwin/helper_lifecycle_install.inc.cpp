bool uninstall_existing_stable_exv() {
  const auto &platform_config = helper_platform_config();
  if (!utils::file_exists(platform_config.stable_install_path))
    return true;

  utils::print_info(
      "Running service uninstall using the existing stable exv before replacement...");
  if (utils::run_command(utils::shell_quote(platform_config.stable_install_path) +
                         " service uninstall") != 0) {
    utils::print_error(
        "Failed to uninstall the existing helper service before replacing /usr/local/bin/exv.");
    return false;
  }
  return true;
}

bool copy_file_contents(const std::string &source_path,
                        const std::string &target_path,
                        int *error_number = nullptr) {
  if (error_number)
    *error_number = 0;

  int src_fd = open(source_path.c_str(), O_RDONLY);
  if (src_fd < 0) {
    if (error_number)
      *error_number = errno;
    return false;
  }

  std::string temp_template = target_path + ".tmp.XXXXXX";
  std::vector<char> temp_path(temp_template.begin(), temp_template.end());
  temp_path.push_back('\0');

  int dst_fd = mkstemp(temp_path.data());
  if (dst_fd < 0) {
    int saved_errno = errno;
    close(src_fd);
    if (error_number)
      *error_number = saved_errno;
    return false;
  }

  bool ok = true;
  int saved_errno = 0;
  char buffer[16384];
  while (ok) {
    ssize_t read_size = read(src_fd, buffer, sizeof(buffer));
    if (read_size == 0)
      break;
    if (read_size < 0) {
      if (errno == EINTR)
        continue;
      saved_errno = errno;
      ok = false;
      break;
    }

    ssize_t total_written = 0;
    while (total_written < read_size) {
      ssize_t write_size =
          write(dst_fd, buffer + total_written, read_size - total_written);
      if (write_size < 0) {
        if (errno == EINTR)
          continue;
        saved_errno = errno;
        ok = false;
        break;
      }
      total_written += write_size;
    }
  }

  if (ok && fsync(dst_fd) != 0) {
    saved_errno = errno;
    ok = false;
  }
  if (ok && chmod(temp_path.data(), 0755) != 0) {
    saved_errno = errno;
    ok = false;
  }

  close(src_fd);
  if (close(dst_fd) != 0 && ok) {
    saved_errno = errno;
    ok = false;
  }

  if (ok && rename(temp_path.data(), target_path.c_str()) != 0) {
    saved_errno = errno;
    ok = false;
  }

  if (!ok) {
    std::remove(temp_path.data());
    if (error_number)
      *error_number = saved_errno;
    errno = saved_errno;
    return false;
  }

  return true;
}

std::string helper_binary_next_to(const std::string &executable_path) {
  std::filesystem::path path(executable_path);
  return (path.parent_path() / "exv-helper").string();
}

} // namespace

void cleanup_routes() {
  tunnel::cleanup_routes();
}

std::string get_interfaces_output() {
  return utils::run_command_output("ifconfig | grep -A 2 'utun' | head -20");
}

void kill_all_supervisors() {
  std::string output = utils::trim(utils::run_command_output("pgrep -f 'exv -rt'"));
  if (output.empty())
    return;

  std::istringstream iss(output);
  std::string line;
  while (std::getline(iss, line)) {
    line = utils::trim(line);
    if (line.empty())
      continue;
    try {
      pid_t pid = static_cast<pid_t>(std::stoi(line));
      if (pid > 0 && is_process_alive(pid)) {
        logger::info("Killing orphaned supervisor: PID " + line);
        kill(pid, SIGKILL);
      }
    } catch (...) {
    }
  }
  usleep(500000);
}

void fix_config_dir_ownership() {
  utils::fix_config_dir_ownership();
}

int copy_self_to_stable_path_and_reexec(const std::string &current_path) {
  const auto &platform_config = helper_platform_config();
  SemanticVersion current_version;
  bool current_version_ok =
      parse_semantic_version(ECNUVPN_VERSION, &current_version);
  bool stable_exists = utils::file_exists(platform_config.stable_install_path);

  utils::print_warning(
      "EXV helper service should be installed from a stable system path.");
  std::cout << utils::DIM << "  Current executable: " << current_path
            << utils::RESET << std::endl;
  std::cout << utils::DIM << "  Stable target: "
            << platform_config.stable_install_path << utils::RESET << std::endl;
  if (current_version_ok) {
    std::cout << utils::DIM << "  Current version: "
              << format_semantic_version(current_version) << utils::RESET
              << std::endl;
  }

  bool proceed = false;
  if (!stable_exists) {
    std::cout << std::endl;
    proceed = prompt_confirm(
        "No exv binary was found at the stable target. Copy this binary there and re-run service installation from that location?",
        true);
  } else {
    SemanticVersion stable_version;
    bool stable_version_ok =
        read_binary_version(platform_config.stable_install_path, &stable_version);
    if (stable_version_ok) {
      std::cout << utils::DIM << "  Existing stable version: "
                << format_semantic_version(stable_version) << utils::RESET
                << std::endl;
    } else {
      std::cout << utils::DIM << "  Existing stable version: unknown"
                << utils::RESET << std::endl;
    }

    std::cout << std::endl;
    if (stable_version_ok && current_version_ok) {
      int cmp = compare_semantic_versions(stable_version, current_version);
      if (cmp == 0) {
        proceed = prompt_confirm(
            "The stable exv already matches this version. Reinstall it and refresh the helper service?",
            false);
      } else if (cmp < 0) {
        proceed = prompt_confirm(
            "The stable exv is older than this build. Upgrade it and reinstall the helper service?",
            true);
      } else {
        proceed = prompt_confirm(
            "The stable exv is newer than this build. Downgrade it and reinstall the helper service?",
            false);
      }
    } else {
      proceed = prompt_confirm(
          "An existing exv was found at the stable target, but its version could not be compared reliably. Replace it and reinstall the helper service?",
          false);
    }
  }

  if (!proceed) {
    utils::print_info("Service installation canceled.");
    return 1;
  }

  if (stable_exists && !uninstall_existing_stable_exv()) {
    return 1;
  }

  if (!utils::ensure_dir("/usr/local") || !utils::ensure_dir("/usr/local/bin")) {
    utils::print_error("Failed to ensure /usr/local/bin exists.");
    return 1;
  }

  utils::print_info("Copying current exv binary to /usr/local/bin/exv ...");
  int copy_error = 0;
  if (!copy_file_contents(current_path, platform_config.stable_install_path,
                          &copy_error)) {
    utils::print_error("Failed to copy exv to /usr/local/bin/exv: " +
                       std::string(std::strerror(copy_error)));
    return 1;
  }

  utils::print_success("Stable exv binary updated at /usr/local/bin/exv.");
  std::string helper_source = helper_binary_next_to(current_path);
  if (!utils::file_exists(helper_source)) {
    utils::print_error("Expected exv-helper next to the current exv binary: " +
                       helper_source);
    return 1;
  }

  utils::print_info("Copying exv-helper binary to /usr/local/bin/exv-helper ...");
  if (!copy_file_contents(helper_source,
                          platform_config.default_service_binary_path,
                          &copy_error)) {
    utils::print_error("Failed to copy exv-helper to /usr/local/bin/exv-helper: " +
                       std::string(std::strerror(copy_error)));
    return 1;
  }
  utils::print_success(
      "Stable exv-helper binary updated at /usr/local/bin/exv-helper.");

  utils::print_info("Re-running service installation from the copied binary...");
  execl(platform_config.stable_install_path, platform_config.stable_install_path,
        "service", "install", static_cast<char *>(nullptr));

  utils::print_error("Failed to launch /usr/local/bin/exv: " +
                     std::string(std::strerror(errno)));
  return 1;
}

