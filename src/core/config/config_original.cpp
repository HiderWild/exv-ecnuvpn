#include "core/config/config.hpp"
#include "core/crypto/crypto.hpp"
#include "common/diagnostics/logger.hpp"
#include "cli/console.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/runtime_paths.hpp"
#include "core/vpn/openconnect_tunnel_script.hpp"
#include "utils/strings.hpp"

#include <algorithm>
#include <cctype>
#include <iomanip>
#include <iostream>
#include <sstream>
#ifndef _WIN32
#include <termios.h>
#include <unistd.h>
#else
#include <windows.h>
#include <conio.h>
#endif

namespace ecnuvpn {
namespace config {
// Begin inlined from core/config/config_wizard_ui include-unit
// Repeat a multi-byte string N times
static std::string repeat_str(const std::string &s, int n) {
  std::string r;
  r.reserve(s.size() * n);
  for (int i = 0; i < n; ++i)
    r += s;
  return r;
}

// ── Wizard TUI helpers ───────────────────────────────────────────

static void wiz_banner() {
  std::cout << std::endl;
  std::cout << cli::BOLD << cli::CYAN
            << "  ╔══════════════════════════════════════════╗" << std::endl
            << "  ║          EXV First-Run Setup             ║" << std::endl
            << "  ╚══════════════════════════════════════════╝" << cli::RESET
            << std::endl;
  std::cout << std::endl;
}

static void wiz_progress(int step, int total) {
  constexpr int BAR = 24;
  int filled = (step * BAR) / total;
  std::cout << cli::DIM << "  Progress: [" << cli::RESET << cli::CYAN;
  for (int i = 0; i < BAR; ++i)
    std::cout << (i < filled ? "█" : "░");
  std::cout << cli::RESET << cli::DIM << "]  " << step << "/" << total
            << cli::RESET << std::endl
            << std::endl;
}

static void wiz_step(int step, int total, const std::string &title) {
  std::cout << std::endl;
  std::cout << cli::BOLD << cli::YELLOW << "  ┌ Step " << step << " / "
            << total << " ─ " << title << cli::RESET << std::endl;
  wiz_progress(step, total);
}

static std::string wiz_prompt(const std::string &label,
                              const std::string &default_val) {
  std::cout << "    " << label;
  if (!default_val.empty())
    std::cout << cli::DIM << " [" << default_val << "]" << cli::RESET;
  std::cout << ": ";
  std::string input;
  std::getline(std::cin, input);
  input = exv::utils::trim(input);
  return input.empty() ? default_val : input;
}

static bool wiz_confirm(const std::string &question, bool default_yes = true) {
  std::cout << "    " << question << (default_yes ? " [Y/n]: " : " [y/N]: ");
  std::string input;
  std::getline(std::cin, input);
  input = exv::utils::trim(input);
  if (input.empty())
    return default_yes;
  return (input[0] == 'y' || input[0] == 'Y');
}

// ── CIDR validation ──────────────────────────────────────────────

static bool is_valid_cidr(const std::string &s) {
  if (s.empty())
    return false;
  if (s.back() == '.' || s.back() == '/')
    return false;
  std::string ip_part = s;
  auto slash = s.find('/');
  if (slash != std::string::npos) {
    std::string pstr = s.substr(slash + 1);
    if (pstr.empty())
      return false;
    int prefix;
    try {
      prefix = std::stoi(pstr);
    } catch (...) {
      return false;
    }
    if (prefix < 0 || prefix > 32)
      return false;
    if (pstr.size() > 1 && pstr[0] == '0')
      return false;
    ip_part = s.substr(0, slash);
  }
  int octets = 0;
  std::istringstream iss(ip_part);
  std::string octet;
  while (std::getline(iss, octet, '.')) {
    if (octet.empty())
      return false;
    for (char c : octet)
      if (!std::isdigit(static_cast<unsigned char>(c)))
        return false;
    int v;
    try {
      v = std::stoi(octet);
    } catch (...) {
      return false;
    }
    if (v < 0 || v > 255)
      return false;
    ++octets;
  }
  return octets == 4;
}

// ── Raw keypress reading ─────────────────────────────────────────

enum RawKey {
  KEY_NONE = 0,
  KEY_UP,
  KEY_DOWN,
  KEY_LEFT,
  KEY_RIGHT,
  KEY_SPACE,
  KEY_ENTER,
  KEY_BACKSPACE,
  KEY_DELETE,
  KEY_PRINTABLE
};

struct KeyEvent {
  RawKey type = KEY_NONE;
  char ch = 0;
};

static KeyEvent read_key_raw() {
#ifndef _WIN32
  unsigned char c;
  if (read(STDIN_FILENO, &c, 1) != 1)
    return {KEY_NONE, 0};
  if (c == '\n' || c == '\r')
    return {KEY_ENTER, 0};
  if (c == ' ')
    return {KEY_SPACE, 0};
  if (c == 127 || c == 8)
    return {KEY_BACKSPACE, 0};
  if (c == 27) {
    unsigned char seq[2];
    if (read(STDIN_FILENO, &seq[0], 1) != 1)
      return {KEY_NONE, 0};
    if (read(STDIN_FILENO, &seq[1], 1) != 1)
      return {KEY_NONE, 0};
    if (seq[0] == '[') {
      switch (seq[1]) {
      case 'A':
        return {KEY_UP, 0};
      case 'B':
        return {KEY_DOWN, 0};
      case 'C':
        return {KEY_RIGHT, 0};
      case 'D':
        return {KEY_LEFT, 0};
      case '3': {
        unsigned char tilde;
        if (read(STDIN_FILENO, &tilde, 1) == 1) {
        }
        return {KEY_DELETE, 0};
      }
      }
    }
    return {KEY_NONE, 0};
  }
  if (c >= 32 && c < 127)
    return {KEY_PRINTABLE, static_cast<char>(c)};
  return {KEY_NONE, 0};
#else
  // Windows: use _getch() from conio.h
  int c = _getch();
  if (c == '\r' || c == '\n')
    return {KEY_ENTER, 0};
  if (c == ' ')
    return {KEY_SPACE, 0};
  if (c == 8 || c == 127)
    return {KEY_BACKSPACE, 0};
  if (c == 0 || c == 0xE0) {
    // Extended key - read the second byte
    int ext = _getch();
    switch (ext) {
    case 72: return {KEY_UP, 0};
    case 80: return {KEY_DOWN, 0};
    case 75: return {KEY_LEFT, 0};
    case 77: return {KEY_RIGHT, 0};
    case 83: return {KEY_DELETE, 0};
    }
    return {KEY_NONE, 0};
  }
  if (c >= 32 && c < 127)
    return {KEY_PRINTABLE, static_cast<char>(c)};
  return {KEY_NONE, 0};
#endif
}

// ── Route selector state ─────────────────────────────────────────

struct RouteSelState {
  std::vector<std::string> defaults;
  std::vector<bool> def_sel;
  std::vector<std::string> custom;
  std::vector<bool> cust_sel;

  int active_col = 0;
  int cursor = 0;
  int scroll_l = 0;
  int scroll_r = 0;
  std::string input;
  std::string status;
  std::string status_color;
  bool done = false;

  static constexpr int PAGE = 10;

  int left_size() const { return static_cast<int>(defaults.size()); }
  int right_size() const { return static_cast<int>(custom.size()); }
  int cur_size() const { return active_col == 0 ? left_size() : right_size(); }
  int &cur_scroll() { return active_col == 0 ? scroll_l : scroll_r; }

  void clamp_cursor() {
    int sz = cur_size();
    if (sz == 0) {
      cursor = 0;
      return;
    }
    if (cursor < 0)
      cursor = 0;
    if (cursor >= sz)
      cursor = sz - 1;
    int &scr = cur_scroll();
    if (cursor < scr)
      scr = cursor;
    if (cursor >= scr + PAGE)
      scr = cursor - PAGE + 1;
  }

  void set_status(const std::string &msg, const std::string &color = "") {
    status = msg;
    status_color = color.empty() ? std::string(cli::DIM) : color;
  }
  void clear_status() { status.clear(); }
};

static int rendered_lines = 0;

static void render_tui(RouteSelState &st) {
  if (rendered_lines > 0)
    std::cout << "\033[" << rendered_lines << "A\033[J";

  int lines = 0;

  // Status line (1 line always)
  if (!st.status.empty())
    std::cout << "    " << st.status_color << st.status << cli::RESET
              << std::endl;
  else
    std::cout << std::endl;
  ++lines;

  // Header
  std::string lhdr =
      st.active_col == 0
          ? (std::string(cli::BOLD) + cli::UNDERLINE + "Default Routes" +
             cli::RESET)
          : (std::string(cli::BOLD) + "Default Routes" + cli::RESET);
  std::string rhdr =
      st.active_col == 1
          ? (std::string(cli::BOLD) + cli::UNDERLINE + "Custom Routes" +
             cli::RESET)
          : (std::string(cli::BOLD) + "Custom Routes" + cli::RESET);
  // Pad left header to 34 visible chars
  std::cout << "    " << lhdr;
  int lpad = 34 - 14; // "Default Routes" = 14 chars
  for (int p = 0; p < lpad; ++p)
    std::cout << ' ';
  std::cout << "│  " << rhdr << std::endl;
  std::cout << "    " << repeat_str("─", 34) << "┼" << repeat_str("─", 30)
            << std::endl;
  lines += 2;

  // Top scroll indicators
  bool lup = st.scroll_l > 0;
  bool rup = st.scroll_r > 0;
  if (lup || rup) {
    std::cout << "    ";
    if (lup) {
      std::string t = "▲ " + std::to_string(st.scroll_l) + " more above";
      std::cout << cli::DIM << t << cli::RESET;
      int pad = 34 - static_cast<int>(t.size());
      for (int p = 0; p < pad; ++p)
        std::cout << ' ';
    } else {
      std::cout << std::string(34, ' ');
    }
    std::cout << "│  ";
    if (rup)
      std::cout << cli::DIM << "▲ " << st.scroll_r << " more above"
                << cli::RESET;
    std::cout << std::endl;
    ++lines;
  }

  // Rows
  int lend = std::min(st.scroll_l + RouteSelState::PAGE, st.left_size());
  int rend = std::min(st.scroll_r + RouteSelState::PAGE, st.right_size());
  int vis_l = lend - st.scroll_l;
  int vis_r = rend - st.scroll_r;
  int max_rows = std::max(vis_l, vis_r);
  if (max_rows == 0)
    max_rows = 1;

  for (int row = 0; row < max_rows; ++row) {
    std::cout << "    ";
    int li = st.scroll_l + row;
    int ri = st.scroll_r + row;

    // Left column
    if (li < st.left_size()) {
      bool cur = (st.active_col == 0 && li == st.cursor);
      std::string ck = st.def_sel[li]
                           ? (std::string(cli::GREEN) + "[x]" + cli::RESET)
                           : (std::string(cli::DIM) + "[ ]" + cli::RESET);
      std::string lb = st.defaults[li];
      int vlen = 4 + static_cast<int>(lb.size());
      int pad = 34 - vlen;
      if (pad < 0)
        pad = 0;

      if (cur)
        std::cout << cli::REVERSE;
      std::cout << ck << " " << lb;
      for (int p = 0; p < pad; ++p)
        std::cout << ' ';
      if (cur)
        std::cout << cli::RESET;
    } else if (li == 0 && st.left_size() == 0) {
      std::cout << cli::DIM << "(empty)" << cli::RESET
                << std::string(27, ' ');
    } else {
      std::cout << std::string(34, ' ');
    }

    std::cout << "│  ";

    // Right column
    if (ri < st.right_size()) {
      bool cur = (st.active_col == 1 && ri == st.cursor);
      std::string ck = st.cust_sel[ri]
                           ? (std::string(cli::CYAN) + "[x]" + cli::RESET)
                           : (std::string(cli::DIM) + "[ ]" + cli::RESET);
      std::string lb = st.custom[ri];
      if (cur)
        std::cout << cli::REVERSE;
      std::cout << ck << " " << lb;
      if (cur)
        std::cout << cli::RESET;
    } else if (ri == 0 && st.right_size() == 0) {
      std::cout << cli::DIM << "(empty)" << cli::RESET;
    }
    std::cout << std::endl;
    ++lines;
  }

  // Bottom scroll indicators
  bool ldown = lend < st.left_size();
  bool rdown = rend < st.right_size();
  if (ldown || rdown) {
    std::cout << "    ";
    if (ldown) {
      int rem = st.left_size() - lend;
      std::string t = "▼ " + std::to_string(rem) + " more below";
      std::cout << cli::DIM << t << cli::RESET;
      int pad = 34 - static_cast<int>(t.size());
      for (int p = 0; p < pad; ++p)
        std::cout << ' ';
    } else {
      std::cout << std::string(34, ' ');
    }
    std::cout << "│  ";
    if (rdown) {
      int rem = st.right_size() - rend;
      std::cout << cli::DIM << "▼ " << rem << " more below" << cli::RESET;
    }
    std::cout << std::endl;
    ++lines;
  }

  // Footer
  std::cout << "    " << repeat_str("─", 34) << "┴" << repeat_str("─", 30)
            << std::endl;
  ++lines;

  // Help + input
  std::cout << cli::DIM << "    ↑↓ Navigate  ←→ Switch column  "
            << "Space Toggle  Enter Add/Done" << cli::RESET << std::endl;
  ++lines;
  std::cout << "    " << cli::BOLD << ">" << cli::RESET << " " << st.input
            << std::flush;
  ++lines;

  rendered_lines = lines;
}
// End inlined from core/config/config_wizard_ui include-unit
// Begin inlined from core/config/config_wizard_routes include-unit
static std::vector<std::string>
wiz_route_selector(const std::vector<std::string> &default_routes) {
  RouteSelState st;
  st.defaults = default_routes;
  st.def_sel.assign(st.defaults.size(), true);

#ifndef _WIN32
  struct termios oldt, newt;
  tcgetattr(STDIN_FILENO, &oldt);
  newt = oldt;
  newt.c_lflag &= ~(ICANON | ECHO);
  newt.c_cc[VMIN] = 1;
  newt.c_cc[VTIME] = 0;
  tcsetattr(STDIN_FILENO, TCSANOW, &newt);
#else
  HANDLE hStdin = GetStdHandle(STD_INPUT_HANDLE);
  DWORD old_console_mode = 0;
  if (hStdin != INVALID_HANDLE_VALUE) {
    GetConsoleMode(hStdin, &old_console_mode);
    SetConsoleMode(hStdin, old_console_mode & ~(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT));
  }
#endif

  rendered_lines = 0;
  std::cout << std::endl;
  render_tui(st);

  while (!st.done) {
    KeyEvent ev = read_key_raw();
    st.clear_status();

    switch (ev.type) {
    case KEY_UP:
      if (st.cur_size() > 0) {
        --st.cursor;
        st.clamp_cursor();
      }
      break;
    case KEY_DOWN:
      if (st.cur_size() > 0) {
        ++st.cursor;
        st.clamp_cursor();
      }
      break;
    case KEY_LEFT:
      if (st.active_col == 1) {
        st.active_col = 0;
        st.cursor = std::min(st.cursor, std::max(0, st.left_size() - 1));
        st.clamp_cursor();
      }
      break;
    case KEY_RIGHT:
      if (st.active_col == 0 && st.right_size() > 0) {
        st.active_col = 1;
        st.cursor = std::min(st.cursor, std::max(0, st.right_size() - 1));
        st.clamp_cursor();
      }
      break;
    case KEY_SPACE:
      if (st.active_col == 0 && st.cursor < st.left_size())
        st.def_sel[st.cursor] = !st.def_sel[st.cursor];
      else if (st.active_col == 1 && st.cursor < st.right_size())
        st.cust_sel[st.cursor] = !st.cust_sel[st.cursor];
      break;
    case KEY_ENTER:
      if (st.input.empty()) {
        st.done = true;
      } else {
        std::string route = exv::utils::trim(st.input);
        st.input.clear();
        if (!route.empty()) {
          if (!is_valid_cidr(route)) {
            st.set_status("Invalid format: " + route +
                              "  (use IP or IP/CIDR e.g. 10.0.0.0/8)",
                          cli::RED);
          } else {
            bool dup = false;
            for (auto &r : st.defaults)
              if (r == route) {
                dup = true;
                break;
              }
            if (!dup)
              for (auto &r : st.custom)
                if (r == route) {
                  dup = true;
                  break;
                }
            if (dup) {
              st.set_status("Route already exists: " + route, cli::YELLOW);
            } else {
              st.custom.push_back(route);
              st.cust_sel.push_back(true);
              st.active_col = 1;
              st.cursor = st.right_size() - 1;
              st.clamp_cursor();
              st.set_status("Added: " + route, cli::GREEN);
            }
          }
        }
      }
      break;
    case KEY_BACKSPACE:
    case KEY_DELETE:
      if (!st.input.empty())
        st.input.pop_back();
      break;
    case KEY_PRINTABLE:
      st.input += ev.ch;
      break;
    default:
      break;
    }
    if (!st.done)
      render_tui(st);
  }

#ifndef _WIN32
  tcsetattr(STDIN_FILENO, TCSANOW, &oldt);
#else
  if (hStdin != INVALID_HANDLE_VALUE) {
    SetConsoleMode(hStdin, old_console_mode);
  }
#endif
  std::cout << std::endl;

  std::vector<std::string> result;
  for (size_t i = 0; i < st.defaults.size(); ++i)
    if (st.def_sel[i])
      result.push_back(st.defaults[i]);
  for (size_t i = 0; i < st.custom.size(); ++i)
    if (st.cust_sel[i])
      result.push_back(st.custom[i]);
  return result;
}
// End inlined from core/config/config_wizard_routes include-unit
// Begin inlined from core/config/config_wizard_flow include-unit
// ── Setup wizard ─────────────────────────────────────────────────

static Config run_wizard() {
  wiz_banner();

  std::cout << "  No configuration found!" << std::endl;
  std::cout << "  Let's get you set up." << std::endl << std::endl;

  std::cout << "  Choose a setup mode:" << std::endl;
  std::cout << "    " << cli::GREEN << "[1]" << cli::RESET
            << " Easy Mode     — quick setup with defaults  " << cli::DIM
            << "(recommended)" << cli::RESET << std::endl;
  std::cout << "    " << cli::YELLOW << "[2]" << cli::RESET
            << " Advanced Mode — customize all settings" << std::endl;
  std::cout << std::endl << "  Choice [1]: ";
  std::string mode_input;
  std::getline(std::cin, mode_input);
  mode_input = exv::utils::trim(mode_input);
  bool advanced = (!mode_input.empty() && mode_input[0] == '2');
  std::cout << std::endl;

  Config cfg;

  if (!advanced) {
    wiz_step(1, 2, "Account");
    cfg.username = wiz_prompt("Username (student ID)", cfg.username);

    wiz_step(2, 2, "Password");
    cli::print_info("Password input is hidden and will not be displayed.");
    cfg.password = crypto::read_password_hidden("    Password: ");
    wiz_progress(2, 2);

    crypto::init_key_if_needed();
    if (!cfg.password.empty()) {
      std::string key = crypto::load_key();
      cfg.password = crypto::encrypt(cfg.password, key);
      cfg.remember_password = true;
    }

  } else {
    constexpr int TOTAL = 6;

    wiz_step(1, TOTAL, "Working Directory");
    std::cout << "    Where should exv store its files?" << std::endl;
    std::string default_dir = platform::get_config_dir();
    std::string new_dir = wiz_prompt("Directory", default_dir);
    if (new_dir != default_dir) {
      if (!platform::set_config_dir(new_dir))
        cli::print_warning("Could not create " + new_dir +
                             ". Using default.");
      else
        cli::print_success("Work directory: " + platform::expand_home(new_dir));
    }
    cfg.log_file = new_dir + "/ecnuvpn.log";

    wiz_step(2, TOTAL, "VPN Server");
    cfg.server = wiz_prompt("Server URL", cfg.server);

    wiz_step(3, TOTAL, "Account");
    cfg.username = wiz_prompt("Username (student ID)", cfg.username);

    wiz_step(4, TOTAL, "Remember Password");
    std::cout << "    Should exv save your password (encrypted)?"
              << std::endl;
    std::cout
        << "    Choosing no means you will be prompted every time you connect."
        << std::endl;
    cfg.remember_password = wiz_confirm("Remember password?", true);

    crypto::init_key_if_needed();

    wiz_step(5, TOTAL, "Split Tunnel Routes");
    std::cout << "    Toggle numbers to select/deselect routes." << std::endl;
    std::cout
        << "    Enter an IP or CIDR (e.g. 10.0.0.0/8) to add a custom route."
        << std::endl;
    std::cout << "    Press " << cli::BOLD << "Enter" << cli::RESET
              << " on an empty line to confirm." << std::endl;
    cfg.routes = wiz_route_selector(cfg.routes);
    std::cout << std::endl;
    cli::print_success("Routes configured: " +
                         std::to_string(cfg.routes.size()) + " selected.");

    wiz_step(6, TOTAL, "Password");
    if (cfg.remember_password) {
      cli::print_info("Password input is hidden and will not be displayed.");
      std::string pw = crypto::read_password_hidden("    Password: ");
      if (!pw.empty()) {
        std::string key = crypto::load_key();
        cfg.password = crypto::encrypt(pw, key);
      }
    } else {
      cli::print_info("Password will be prompted each time you connect.");
      cfg.password = "";
    }
    wiz_progress(TOTAL, TOTAL);
  }

  std::cout << std::endl;
  cli::print_success("Setup complete!");
  std::cout << std::endl;
  return cfg;
}
// End inlined from core/config/config_wizard_flow include-unit
// Begin inlined from core/config/config_persistence_legacy include-unit
// ── Load / Save ──────────────────────────────────────────────────

Config load() {
  std::string dir = platform::get_config_dir();
  std::string path = platform::get_config_path();
  platform::ensure_dir(dir);

  if (!platform::fix_runtime_config_dir_ownership()) {
    cli::print_error("Configuration directory is owned by another user: " + dir);
    cli::print_info("Fix with: sudo chown -R $(whoami) " + dir);
    logger::error("Config dir ownership mismatch: " + dir);
    return Config{};
  }

  if (!platform::file_exists(path)) {
    Config cfg = run_wizard();
    if (!save(cfg)) {
      cli::print_error("Failed to save configuration to: " + path);
      cli::print_warning("Check directory permissions: " + dir);
      cli::print_info("If ~/.ecnuvpn is owned by root, fix with: sudo chown -R $(whoami) ~/.ecnuvpn");
      logger::error("Config save failed after wizard");
    }
    crypto::init_key_if_needed();
    logger::info("First-run setup wizard completed");
    return cfg;
  }

  crypto::init_key_if_needed();

  try {
    std::string content = platform::read_file(path);
    auto j = nlohmann::json::parse(content);
    return j.get<Config>();
  } catch (const std::exception &e) {
    cli::print_error("Failed to parse config: " + std::string(e.what()));
    cli::print_warning("Using default config.");
    logger::error("Config parse error: " + std::string(e.what()));
    return Config{};
  }
}

bool save(const Config &cfg) {
  std::string dir = platform::get_config_dir();
  std::string path = platform::get_config_path();
  platform::ensure_dir(dir);
  try {
    nlohmann::json j = cfg;
    if (platform::write_file(path, j.dump(4))) {
      logger::info("Config saved to: " + path);
      return true;
    }
  } catch (const std::exception &e) {
    cli::print_error("Failed to save config: " + std::string(e.what()));
    logger::error("Config save error: " + std::string(e.what()));
  }
  return false;
}
// End inlined from core/config/config_persistence_legacy include-unit
// Begin inlined from core/config/config_show_legacy include-unit
// ── Show ────────────────────────────────────────────────────────

void show(const Config &cfg) {
  cli::print_header("EXV Configuration");

  auto pw_status = [&]() -> std::string {
    if (!cfg.remember_password)
      return std::string(cli::DIM) + "(prompt at connect)" + cli::RESET;
    if (cfg.password.empty())
      return "(not set — run: exv config set password)";
    std::string ks = crypto::key_status();
    if (ks == "valid")
      return std::string(cli::GREEN) + "stored (encrypted)" + cli::RESET;
    return std::string(cli::RED) + "[KEY " + ks +
           " — run: exv config key reset]" + cli::RESET;
  };

  std::cout << cli::BOLD << "  Server          : " << cli::RESET
            << cfg.server << std::endl;
  std::cout << cli::BOLD << "  Username        : " << cli::RESET
            << (cfg.username.empty() ? "(not set)" : cfg.username) << std::endl;
  std::cout << cli::BOLD << "  Password        : " << cli::RESET
            << pw_status() << std::endl;
  std::cout << cli::BOLD << "  Remember Passwd : " << cli::RESET
            << (cfg.remember_password
                    ? std::string(cli::GREEN) + "yes" + cli::RESET
                    : std::string(cli::YELLOW) + "no (prompt on connect)" +
                          cli::RESET)
            << std::endl;
  std::cout << cli::BOLD << "  MTU             : " << cli::RESET << cfg.mtu
            << std::endl;
  std::cout << cli::BOLD << "  UserAgent       : " << cli::RESET
            << cfg.useragent << std::endl;
  std::cout << cli::BOLD << "  Disable DTLS    : " << cli::RESET
            << (cfg.disable_dtls
                    ? std::string(cli::YELLOW) + "yes (TLS-only transport)" +
                          cli::RESET
                    : std::string(cli::GREEN) + "no" + cli::RESET)
            << std::endl;
  std::cout << cli::BOLD << "  Log File        : " << cli::RESET
            << cfg.log_file << std::endl;
  std::cout << cli::BOLD << "  Auto Reconnect  : " << cli::RESET
            << (cfg.auto_reconnect ? "true" : "false") << std::endl;
  std::cout << cli::BOLD << "  Minimal Mode    : " << cli::RESET
            << (cfg.minimal_mode ? "true" : "false") << std::endl;
  std::cout << cli::BOLD << "  Minimal Install Service: " << cli::RESET
            << (cfg.minimal_install_service_before_connect ? "true" : "false")
            << std::endl;
  std::cout << cli::BOLD << "  VPN Engine      : " << cli::RESET
            << cfg.vpn_engine << std::endl;
  std::cout << cli::BOLD << "  OpenConnect Runtime: " << cli::RESET
            << cfg.openconnect_runtime << std::endl;
#ifdef _WIN32
  std::cout << cli::BOLD << "  Tunnel Driver   : " << cli::RESET
            << cfg.windows_tunnel_driver << std::endl;
  std::cout << cli::BOLD << "  TAP Interface   : " << cli::RESET
            << (cfg.windows_tap_interface.empty() ? "(auto)"
                                                  : cfg.windows_tap_interface)
            << std::endl;
#endif
  std::cout << std::endl;

  std::cout << cli::BOLD << "  Routes (" << cfg.routes.size()
            << "):" << cli::RESET << std::endl;
  for (const auto &r : cfg.routes)
    std::cout << "    • " << r << std::endl;

  if (!cfg.extra_args.empty()) {
    std::cout << std::endl
              << cli::BOLD << "  Extra Args:" << cli::RESET << std::endl;
    for (const auto &a : cfg.extra_args)
      std::cout << "    • " << a << std::endl;
  }

  std::cout << std::endl;
  std::cout << cli::DIM << "  Config : " << platform::get_config_path()
            << cli::RESET << std::endl;
  std::cout << cli::DIM << "  Key    : " << crypto::key_path() << "  ["
            << crypto::key_status() << "]" << cli::RESET << std::endl;
  std::cout << std::endl;
}

// ── get_plaintext_password ───────────────────────────────────────

std::string get_plaintext_password(const Config &cfg) {
  if (!cfg.remember_password) {
    cli::print_info("Password input is hidden and will not be displayed.");
    std::string pw = crypto::read_password_hidden("  VPN Password: ");
    if (pw.empty())
      cli::print_error("Password cannot be empty.");
    return pw;
  }
  if (cfg.password.empty()) {
    cli::print_error("Password not set. Run: exv config set password");
    return "";
  }
  std::string ks = crypto::key_status();
  if (ks != "valid") {
    cli::print_error("Encryption key is " + ks + "!");
    cli::print_info("Run 'exv config key reset' then re-set password.");
    logger::error("Password decrypt failed: key is " + ks);
    return "";
  }
  std::string key = crypto::load_key();
  std::string plaintext = crypto::decrypt(cfg.password, key);
  if (plaintext.empty()) {
    cli::print_error("Failed to decrypt password.");
    cli::print_info("Run 'exv config key reset' then re-set password.");
    logger::error("Password decryption returned empty");
  }
  return plaintext;
}
// End inlined from core/config/config_show_legacy include-unit
// Begin inlined from core/config/config_import_legacy include-unit
// ── Import ──────────────────────────────────────────────────────

Config import_from(const std::string &path) {
  if (!platform::file_exists(path)) {
    cli::print_error("Import file not found: " + path);
    return load();
  }
  try {
    std::string content = platform::read_file(path);
    auto j = nlohmann::json::parse(content);
    Config cfg = load();

    if (j.contains("server"))
      cfg.server = j["server"].get<std::string>();
    if (j.contains("username"))
      cfg.username = j["username"].get<std::string>();
    if (j.contains("mtu"))
      cfg.mtu = j["mtu"].get<int>();
    if (j.contains("useragent"))
      cfg.useragent = j["useragent"].get<std::string>();
    if (j.contains("disable_dtls"))
      cfg.disable_dtls = j["disable_dtls"].get<bool>();
    if (j.contains("routes"))
      cfg.routes = j["routes"].get<std::vector<std::string>>();
    if (j.contains("extra_args"))
      cfg.extra_args = j["extra_args"].get<std::vector<std::string>>();
    if (j.contains("log_file"))
      cfg.log_file = j["log_file"].get<std::string>();
    if (j.contains("remember_password"))
      cfg.remember_password = j["remember_password"].get<bool>();
    if (j.contains("vpn_engine"))
      cfg.vpn_engine = j["vpn_engine"].get<std::string>();
    if (j.contains("openconnect_runtime"))
      cfg.openconnect_runtime = j["openconnect_runtime"].get<std::string>();
    if (j.contains("windows_tunnel_driver"))
      cfg.windows_tunnel_driver =
          j["windows_tunnel_driver"].get<std::string>();
    if (j.contains("windows_tap_interface"))
      cfg.windows_tap_interface =
          j["windows_tap_interface"].get<std::string>();
    if (j.contains("auto_reconnect"))
      cfg.auto_reconnect = j["auto_reconnect"].get<bool>();
    if (j.contains("minimal_mode"))
      cfg.minimal_mode = j["minimal_mode"].get<bool>();
    if (j.contains("service_install_prompt_seen"))
      cfg.service_install_prompt_seen =
          j["service_install_prompt_seen"].get<bool>();
    if (j.contains("minimal_install_service_before_connect"))
      cfg.minimal_install_service_before_connect =
          j["minimal_install_service_before_connect"].get<bool>();

    if (j.contains("password")) {
      std::string pw = j["password"].get<std::string>();
      if (!pw.empty() && cfg.remember_password) {
        std::string ks = crypto::key_status();
        if (ks == "valid") {
          cfg.password = crypto::encrypt(pw, crypto::load_key());
          cli::print_info("Password from import file encrypted and stored.");
        } else {
          cli::print_warning("Key is " + ks +
                               " — password from import NOT stored.");
        }
      }
    }

    save(cfg);
    cli::print_success("Config imported from: " + path);
    logger::info("Config imported from: " + path);
    return cfg;
  } catch (const std::exception &e) {
    cli::print_error("Failed to import: " + std::string(e.what()));
    logger::error("Config import error: " + std::string(e.what()));
    return load();
  }
}
// End inlined from core/config/config_import_legacy include-unit
// Begin inlined from core/config/config_set_value_legacy include-unit
// ── set_value ────────────────────────────────────────────────────

bool set_value(Config &cfg, const std::string &key, const std::string &inline_value) {
  // Strip surrounding quotes that Windows CMD/PowerShell may add.
  auto strip_quotes = [](const std::string &s) -> std::string {
    if (s.size() >= 2 && s.front() == '"' && s.back() == '"')
      return s.substr(1, s.size() - 2);
    return s;
  };
  std::string value = strip_quotes(inline_value);

  // Helper: use inline value if provided, otherwise prompt from stdin.
  auto read_value = [&](const std::string &prompt) -> std::string {
    if (!value.empty())
      return value;
    std::cout << prompt;
    std::string val;
    std::getline(std::cin, val);
    return strip_quotes(exv::utils::trim(val));
  };

  if (key == "password") {
    if (!cfg.remember_password) {
      cli::print_warning("remember_password is currently disabled.");
      cli::print_info(
          "To store an encrypted password, it must be enabled first.");
      std::cout << std::endl;
      std::cout << "  Enable remember_password and set a password now? [Y/n]: ";
      std::string ans;
      std::getline(std::cin, ans);
      ans = exv::utils::trim(ans);
      if (!ans.empty() && ans[0] != 'y' && ans[0] != 'Y') {
        cli::print_info(
            "Aborted. Password will continue to be prompted at connect time.");
        return false;
      }
      cfg.remember_password = true;
      cli::print_success("remember_password enabled.");
    }
    std::string ks = crypto::key_status();
    if (ks != "valid") {
      cli::print_error("Encryption key is " + ks + "!");
      cli::print_info("Run 'exv config key reset' to fix this.");
      return false;
    }
    cli::print_info("Password input is hidden and will not be displayed.");
    std::string pw = crypto::read_password_hidden("  New password: ");
    if (pw.empty()) {
      cli::print_error("Password cannot be empty.");
      return false;
    }
    cfg.password = crypto::encrypt(pw, crypto::load_key());
    if (cfg.password.empty()) {
      cli::print_error("Encryption failed.");
      return false;
    }
    if (save(cfg)) {
      cli::print_success("Password set and encrypted.");
      logger::info("Password updated (encrypted)");
      return true;
    }
    return false;
  }

  if (key == "remember_password") {
    std::string input = read_value("  Remember password? [Y/n]: ");
    if (input.empty())
      input = "y";
    cfg.remember_password = (input[0] == 'y' || input[0] == 'Y');
    if (!cfg.remember_password) {
      cfg.password = "";
      crypto::delete_key_file();
    }
    if (save(cfg)) {
      cli::print_success(std::string("remember_password = ") +
                           (cfg.remember_password ? "true" : "false"));
      return true;
    }
    return false;
  }

  if (key == "disable_dtls") {
    std::string input = read_value("  Disable DTLS? [y/N]: ");
    cfg.disable_dtls = (!input.empty() && (input[0] == 'y' || input[0] == 'Y'));
    if (save(cfg)) {
      cli::print_success(std::string("disable_dtls = ") +
                           (cfg.disable_dtls ? "true" : "false"));
      return true;
    }
    return false;
  }

  auto handle_bool = [&](const std::string &k, bool &field,
                         const std::string &prompt,
                         bool default_yes) -> bool {
    if (key != k)
      return false;
    std::string input = read_value(prompt);
    if (input.empty())
      input = default_yes ? "y" : "n";
    if (input == "true" || input == "1") {
      field = true;
    } else if (input == "false" || input == "0") {
      field = false;
    } else {
      field = (input[0] == 'y' || input[0] == 'Y');
    }
    if (save(cfg)) {
      cli::print_success(k + " = " + (field ? "true" : "false"));
      return true;
    }
    return false;
  };

  if (handle_bool("auto_reconnect", cfg.auto_reconnect,
                  "  Enable auto reconnect? [Y/n]: ", true))
    return true;
  if (handle_bool("minimal_mode", cfg.minimal_mode,
                  "  Enable minimal desktop mode? [Y/n]: ", true))
    return true;
  if (handle_bool("service_install_prompt_seen",
                  cfg.service_install_prompt_seen,
                  "  Mark service install prompt as seen? [y/N]: ", false))
    return true;
  if (handle_bool("minimal_install_service_before_connect",
                  cfg.minimal_install_service_before_connect,
                  "  Install service before minimal-mode connect? [Y/n]: ",
                  true))
    return true;

  if (key == "vpn_engine") {
    std::string input = read_value("  VPN engine [native/legacy_openconnect]: ");
    if (input != "native" && input != "legacy_openconnect") {
      cli::print_error("Invalid VPN engine.");
      return false;
    }
    cfg.vpn_engine = input;
    if (save(cfg)) {
      cli::print_success("Set vpn_engine = " + input);
      return true;
    }
    return false;
  }

  if (key == "openconnect_runtime") {
    std::string input = read_value("  Runtime mode [bundled/auto/system]: ");
    if (input != "bundled" && input != "auto" && input != "system") {
      cli::print_error("Invalid runtime mode.");
      return false;
    }
    cfg.openconnect_runtime = input;
    if (save(cfg)) {
      cli::print_success("Set openconnect_runtime = " + input);
      return true;
    }
    return false;
  }

  if (key == "windows_tunnel_driver") {
    std::string input = read_value("  Tunnel driver [auto/wintun/tap]: ");
    if (input != "auto" && input != "wintun" && input != "tap") {
      cli::print_error("Invalid tunnel driver.");
      return false;
    }
    cfg.windows_tunnel_driver = input;
    if (save(cfg)) {
      cli::print_success("Set windows_tunnel_driver = " + input);
      return true;
    }
    return false;
  }

  auto handle_str = [&](const std::string &k, std::string &field) -> bool {
    if (key != k)
      return false;
    std::string val = read_value("  Enter value for " + k + ": ");
    if (val.empty()) {
      cli::print_error("Value cannot be empty.");
      return false;
    }
    field = val;
    if (save(cfg)) {
      cli::print_success("Set " + k + " = " + val);
      return true;
    }
    return false;
  };

  if (handle_str("server", cfg.server))
    return true;
  if (handle_str("username", cfg.username))
    return true;
  if (handle_str("useragent", cfg.useragent))
    return true;
  if (handle_str("log_file", cfg.log_file))
    return true;
  if (handle_str("windows_tap_interface", cfg.windows_tap_interface))
    return true;

  if (key == "mtu") {
    std::string val = read_value("  Enter value for mtu: ");
    try {
      cfg.mtu = std::stoi(val);
    } catch (...) {
      cli::print_error("Invalid MTU.");
      return false;
    }
    if (save(cfg)) {
      cli::print_success("Set mtu = " + val);
      return true;
    }
    return false;
  }

  cli::print_error("Unknown config key: " + key);
  cli::print_info("Valid keys: server, username, password, mtu, useragent, "
                    "log_file, remember_password, disable_dtls, "
                    "auto_reconnect, minimal_mode, "
                    "service_install_prompt_seen, "
                    "minimal_install_service_before_connect, "
                    "vpn_engine, "
                    "openconnect_runtime, "
                    "windows_tunnel_driver, windows_tap_interface");
  return false;
}
// End inlined from core/config/config_set_value_legacy include-unit
// Begin inlined from core/config/config_maintenance_legacy include-unit
// ── Reset ───────────────────────────────────────────────────────

Config reset() {
  Config cfg;
  save(cfg);
  tunnel::write_script(cfg);
  cli::print_success("Config reset to defaults. Key file preserved.");
  cli::print_info("Run 'exv config set password' to set a new password.");
  logger::info("Config reset to defaults");
  return cfg;
}

// ── Route management ────────────────────────────────────────────

bool add_route(Config &cfg, const std::string &route) {
  if (std::find(cfg.routes.begin(), cfg.routes.end(), route) !=
      cfg.routes.end()) {
    cli::print_warning("Route already exists: " + route);
    return false;
  }
  cfg.routes.push_back(route);
  save(cfg);
  cli::print_success("Route added: " + route);
  logger::info("Route added: " + route);
  return true;
}

bool remove_route(Config &cfg, const std::string &route) {
  auto it = std::find(cfg.routes.begin(), cfg.routes.end(), route);
  if (it == cfg.routes.end()) {
    cli::print_error("Route not found: " + route);
    return false;
  }
  cfg.routes.erase(it);
  save(cfg);
  cli::print_success("Route removed: " + route);
  logger::info("Route removed: " + route);
  return true;
}

void list_routes(const Config &cfg) {
  cli::print_header("VPN Routes");
  if (cfg.routes.empty()) {
    cli::print_warning("No routes configured.");
    return;
  }
  std::cout << "  Total: " << cfg.routes.size() << " routes" << std::endl
            << std::endl;
  for (size_t i = 0; i < cfg.routes.size(); ++i)
    std::cout << "  " << cli::GREEN << (i + 1) << "." << cli::RESET << " "
              << cfg.routes[i] << std::endl;
  std::cout << std::endl;
}

// ── Key management ──────────────────────────────────────────────

void key_show() {
  cli::print_header("Encryption Key Status");
  std::string ks = crypto::key_status();
  std::cout << "  Key file : " << crypto::key_path() << std::endl;
  std::cout << "  Status   : ";
  if (ks == "valid")
    std::cout << cli::GREEN << cli::BOLD << "valid" << cli::RESET
              << std::endl;
  else if (ks == "missing") {
    std::cout << cli::YELLOW << cli::BOLD << "missing" << cli::RESET
              << std::endl;
    cli::print_info("Run: exv config key reset");
  } else {
    std::cout << cli::RED << cli::BOLD << "corrupt" << cli::RESET
              << std::endl;
    cli::print_warning("Run: exv config key reset");
  }
  std::cout << std::endl;
}

bool key_reset() { return crypto::reset_key(); }
// End inlined from core/config/config_maintenance_legacy include-unit
} // namespace config
} // namespace ecnuvpn
