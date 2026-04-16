#include "config.hpp"
#include "crypto.hpp"
#include "logger.hpp"
#include "tunnel.hpp"
#include "utils.hpp"

#include <algorithm>
#include <cctype>
#include <iomanip>
#include <iostream>
#include <sstream>
#include <termios.h>
#include <unistd.h>

namespace ecnuvpn {
namespace config {

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
  std::cout << utils::BOLD << utils::CYAN
            << "  ╔══════════════════════════════════════════╗" << std::endl
            << "  ║          EXV First-Run Setup             ║" << std::endl
            << "  ╚══════════════════════════════════════════╝" << utils::RESET
            << std::endl;
  std::cout << std::endl;
}

static void wiz_progress(int step, int total) {
  constexpr int BAR = 24;
  int filled = (step * BAR) / total;
  std::cout << utils::DIM << "  Progress: [" << utils::RESET << utils::CYAN;
  for (int i = 0; i < BAR; ++i)
    std::cout << (i < filled ? "█" : "░");
  std::cout << utils::RESET << utils::DIM << "]  " << step << "/" << total
            << utils::RESET << std::endl
            << std::endl;
}

static void wiz_step(int step, int total, const std::string &title) {
  std::cout << std::endl;
  std::cout << utils::BOLD << utils::YELLOW << "  ┌ Step " << step << " / "
            << total << " ─ " << title << utils::RESET << std::endl;
  wiz_progress(step, total);
}

static std::string wiz_prompt(const std::string &label,
                              const std::string &default_val) {
  std::cout << "    " << label;
  if (!default_val.empty())
    std::cout << utils::DIM << " [" << default_val << "]" << utils::RESET;
  std::cout << ": ";
  std::string input;
  std::getline(std::cin, input);
  input = utils::trim(input);
  return input.empty() ? default_val : input;
}

static bool wiz_confirm(const std::string &question, bool default_yes = true) {
  std::cout << "    " << question << (default_yes ? " [Y/n]: " : " [y/N]: ");
  std::string input;
  std::getline(std::cin, input);
  input = utils::trim(input);
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
    status_color = color.empty() ? std::string(utils::DIM) : color;
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
    std::cout << "    " << st.status_color << st.status << utils::RESET
              << std::endl;
  else
    std::cout << std::endl;
  ++lines;

  // Header
  std::string lhdr =
      st.active_col == 0
          ? (std::string(utils::BOLD) + utils::UNDERLINE + "Default Routes" +
             utils::RESET)
          : (std::string(utils::BOLD) + "Default Routes" + utils::RESET);
  std::string rhdr =
      st.active_col == 1
          ? (std::string(utils::BOLD) + utils::UNDERLINE + "Custom Routes" +
             utils::RESET)
          : (std::string(utils::BOLD) + "Custom Routes" + utils::RESET);
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
      std::cout << utils::DIM << t << utils::RESET;
      int pad = 34 - static_cast<int>(t.size());
      for (int p = 0; p < pad; ++p)
        std::cout << ' ';
    } else {
      std::cout << std::string(34, ' ');
    }
    std::cout << "│  ";
    if (rup)
      std::cout << utils::DIM << "▲ " << st.scroll_r << " more above"
                << utils::RESET;
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
                           ? (std::string(utils::GREEN) + "[x]" + utils::RESET)
                           : (std::string(utils::DIM) + "[ ]" + utils::RESET);
      std::string lb = st.defaults[li];
      int vlen = 4 + static_cast<int>(lb.size());
      int pad = 34 - vlen;
      if (pad < 0)
        pad = 0;

      if (cur)
        std::cout << utils::REVERSE;
      std::cout << ck << " " << lb;
      for (int p = 0; p < pad; ++p)
        std::cout << ' ';
      if (cur)
        std::cout << utils::RESET;
    } else if (li == 0 && st.left_size() == 0) {
      std::cout << utils::DIM << "(empty)" << utils::RESET
                << std::string(27, ' ');
    } else {
      std::cout << std::string(34, ' ');
    }

    std::cout << "│  ";

    // Right column
    if (ri < st.right_size()) {
      bool cur = (st.active_col == 1 && ri == st.cursor);
      std::string ck = st.cust_sel[ri]
                           ? (std::string(utils::CYAN) + "[x]" + utils::RESET)
                           : (std::string(utils::DIM) + "[ ]" + utils::RESET);
      std::string lb = st.custom[ri];
      if (cur)
        std::cout << utils::REVERSE;
      std::cout << ck << " " << lb;
      if (cur)
        std::cout << utils::RESET;
    } else if (ri == 0 && st.right_size() == 0) {
      std::cout << utils::DIM << "(empty)" << utils::RESET;
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
      std::cout << utils::DIM << t << utils::RESET;
      int pad = 34 - static_cast<int>(t.size());
      for (int p = 0; p < pad; ++p)
        std::cout << ' ';
    } else {
      std::cout << std::string(34, ' ');
    }
    std::cout << "│  ";
    if (rdown) {
      int rem = st.right_size() - rend;
      std::cout << utils::DIM << "▼ " << rem << " more below" << utils::RESET;
    }
    std::cout << std::endl;
    ++lines;
  }

  // Footer
  std::cout << "    " << repeat_str("─", 34) << "┴" << repeat_str("─", 30)
            << std::endl;
  ++lines;

  // Help + input
  std::cout << utils::DIM << "    ↑↓ Navigate  ←→ Switch column  "
            << "Space Toggle  Enter Add/Done" << utils::RESET << std::endl;
  ++lines;
  std::cout << "    " << utils::BOLD << ">" << utils::RESET << " " << st.input
            << std::flush;
  ++lines;

  rendered_lines = lines;
}

static std::vector<std::string>
wiz_route_selector(const std::vector<std::string> &default_routes) {
  RouteSelState st;
  st.defaults = default_routes;
  st.def_sel.assign(st.defaults.size(), true);

  struct termios oldt, newt;
  tcgetattr(STDIN_FILENO, &oldt);
  newt = oldt;
  newt.c_lflag &= ~(ICANON | ECHO);
  newt.c_cc[VMIN] = 1;
  newt.c_cc[VTIME] = 0;
  tcsetattr(STDIN_FILENO, TCSANOW, &newt);

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
        std::string route = utils::trim(st.input);
        st.input.clear();
        if (!route.empty()) {
          if (!is_valid_cidr(route)) {
            st.set_status("Invalid format: " + route +
                              "  (use IP or IP/CIDR e.g. 10.0.0.0/8)",
                          utils::RED);
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
              st.set_status("Route already exists: " + route, utils::YELLOW);
            } else {
              st.custom.push_back(route);
              st.cust_sel.push_back(true);
              st.active_col = 1;
              st.cursor = st.right_size() - 1;
              st.clamp_cursor();
              st.set_status("Added: " + route, utils::GREEN);
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

  tcsetattr(STDIN_FILENO, TCSANOW, &oldt);
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

// ── Setup wizard ─────────────────────────────────────────────────

static Config run_wizard() {
  wiz_banner();

  std::cout << "  No configuration found!" << std::endl;
  std::cout << "  Let's get you set up." << std::endl << std::endl;

  std::cout << "  Choose a setup mode:" << std::endl;
  std::cout << "    " << utils::GREEN << "[1]" << utils::RESET
            << " Easy Mode     — quick setup with defaults  " << utils::DIM
            << "(recommended)" << utils::RESET << std::endl;
  std::cout << "    " << utils::YELLOW << "[2]" << utils::RESET
            << " Advanced Mode — customize all settings" << std::endl;
  std::cout << std::endl << "  Choice [1]: ";
  std::string mode_input;
  std::getline(std::cin, mode_input);
  mode_input = utils::trim(mode_input);
  bool advanced = (!mode_input.empty() && mode_input[0] == '2');
  std::cout << std::endl;

  Config cfg;

  if (!advanced) {
    wiz_step(1, 2, "Account");
    cfg.username = wiz_prompt("Username (student ID)", cfg.username);

    wiz_step(2, 2, "Password");
    utils::print_info("Password input is hidden and will not be displayed.");
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
    std::string new_dir = wiz_prompt("Directory", "~/.ecnuvpn");
    if (new_dir != "~/.ecnuvpn") {
      if (!utils::set_config_dir(new_dir))
        utils::print_warning("Could not create " + new_dir +
                             ". Using default.");
      else
        utils::print_success("Work directory: " + utils::expand_home(new_dir));
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
    std::cout << "    Press " << utils::BOLD << "Enter" << utils::RESET
              << " on an empty line to confirm." << std::endl;
    cfg.routes = wiz_route_selector(cfg.routes);
    std::cout << std::endl;
    utils::print_success("Routes configured: " +
                         std::to_string(cfg.routes.size()) + " selected.");

    wiz_step(6, TOTAL, "Password");
    if (cfg.remember_password) {
      utils::print_info("Password input is hidden and will not be displayed.");
      std::string pw = crypto::read_password_hidden("    Password: ");
      if (!pw.empty()) {
        std::string key = crypto::load_key();
        cfg.password = crypto::encrypt(pw, key);
      }
    } else {
      utils::print_info("Password will be prompted each time you connect.");
      cfg.password = "";
    }
    wiz_progress(TOTAL, TOTAL);
  }

  std::cout << std::endl;
  utils::print_success("Setup complete!");
  std::cout << std::endl;
  return cfg;
}

// ── Load / Save ──────────────────────────────────────────────────

Config load() {
  std::string dir = utils::get_config_dir();
  std::string path = utils::get_config_path();
  utils::ensure_dir(dir);

  if (!utils::file_exists(path)) {
    Config cfg = run_wizard();
    save(cfg);
    crypto::init_key_if_needed();
    logger::info("First-run setup wizard completed");
    return cfg;
  }

  crypto::init_key_if_needed();

  try {
    std::string content = utils::read_file(path);
    auto j = nlohmann::json::parse(content);
    return j.get<Config>();
  } catch (const std::exception &e) {
    utils::print_error("Failed to parse config: " + std::string(e.what()));
    utils::print_warning("Using default config.");
    logger::error("Config parse error: " + std::string(e.what()));
    return Config{};
  }
}

bool save(const Config &cfg) {
  std::string dir = utils::get_config_dir();
  std::string path = utils::get_config_path();
  utils::ensure_dir(dir);
  try {
    nlohmann::json j = cfg;
    if (utils::write_file(path, j.dump(4))) {
      logger::info("Config saved to: " + path);
      return true;
    }
  } catch (const std::exception &e) {
    utils::print_error("Failed to save config: " + std::string(e.what()));
    logger::error("Config save error: " + std::string(e.what()));
  }
  return false;
}

// ── Show ────────────────────────────────────────────────────────

void show(const Config &cfg) {
  utils::print_header("EXV Configuration");

  auto pw_status = [&]() -> std::string {
    if (!cfg.remember_password)
      return std::string(utils::DIM) + "(prompt at connect)" + utils::RESET;
    if (cfg.password.empty())
      return "(not set — run: exv config set password)";
    std::string ks = crypto::key_status();
    if (ks == "valid")
      return std::string(utils::GREEN) + "stored (encrypted)" + utils::RESET;
    return std::string(utils::RED) + "[KEY " + ks +
           " — run: exv config key reset]" + utils::RESET;
  };

  std::cout << utils::BOLD << "  Server          : " << utils::RESET
            << cfg.server << std::endl;
  std::cout << utils::BOLD << "  Username        : " << utils::RESET
            << (cfg.username.empty() ? "(not set)" : cfg.username) << std::endl;
  std::cout << utils::BOLD << "  Password        : " << utils::RESET
            << pw_status() << std::endl;
  std::cout << utils::BOLD << "  Remember Passwd : " << utils::RESET
            << (cfg.remember_password
                    ? std::string(utils::GREEN) + "yes" + utils::RESET
                    : std::string(utils::YELLOW) + "no (prompt on connect)" +
                          utils::RESET)
            << std::endl;
  std::cout << utils::BOLD << "  MTU             : " << utils::RESET << cfg.mtu
            << std::endl;
  std::cout << utils::BOLD << "  UserAgent       : " << utils::RESET
            << cfg.useragent << std::endl;
  std::cout << utils::BOLD << "  Log File        : " << utils::RESET
            << cfg.log_file << std::endl;
  std::cout << std::endl;

  std::cout << utils::BOLD << "  Routes (" << cfg.routes.size()
            << "):" << utils::RESET << std::endl;
  for (const auto &r : cfg.routes)
    std::cout << "    • " << r << std::endl;

  if (!cfg.extra_args.empty()) {
    std::cout << std::endl
              << utils::BOLD << "  Extra Args:" << utils::RESET << std::endl;
    for (const auto &a : cfg.extra_args)
      std::cout << "    • " << a << std::endl;
  }

  std::cout << std::endl;
  std::cout << utils::DIM << "  Config : " << utils::get_config_path()
            << utils::RESET << std::endl;
  std::cout << utils::DIM << "  Key    : " << crypto::key_path() << "  ["
            << crypto::key_status() << "]" << utils::RESET << std::endl;
  std::cout << std::endl;
}

// ── get_plaintext_password ───────────────────────────────────────

std::string get_plaintext_password(const Config &cfg) {
  if (!cfg.remember_password) {
    utils::print_info("Password input is hidden and will not be displayed.");
    std::string pw = crypto::read_password_hidden("  VPN Password: ");
    if (pw.empty())
      utils::print_error("Password cannot be empty.");
    return pw;
  }
  if (cfg.password.empty()) {
    utils::print_error("Password not set. Run: exv config set password");
    return "";
  }
  std::string ks = crypto::key_status();
  if (ks != "valid") {
    utils::print_error("Encryption key is " + ks + "!");
    utils::print_info("Run 'exv config key reset' then re-set password.");
    logger::error("Password decrypt failed: key is " + ks);
    return "";
  }
  std::string key = crypto::load_key();
  std::string plaintext = crypto::decrypt(cfg.password, key);
  if (plaintext.empty()) {
    utils::print_error("Failed to decrypt password.");
    utils::print_info("Run 'exv config key reset' then re-set password.");
    logger::error("Password decryption returned empty");
  }
  return plaintext;
}

// ── Import ──────────────────────────────────────────────────────

Config import_from(const std::string &path) {
  if (!utils::file_exists(path)) {
    utils::print_error("Import file not found: " + path);
    return load();
  }
  try {
    std::string content = utils::read_file(path);
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
    if (j.contains("routes"))
      cfg.routes = j["routes"].get<std::vector<std::string>>();
    if (j.contains("extra_args"))
      cfg.extra_args = j["extra_args"].get<std::vector<std::string>>();
    if (j.contains("log_file"))
      cfg.log_file = j["log_file"].get<std::string>();
    if (j.contains("remember_password"))
      cfg.remember_password = j["remember_password"].get<bool>();

    if (j.contains("password")) {
      std::string pw = j["password"].get<std::string>();
      if (!pw.empty() && cfg.remember_password) {
        std::string ks = crypto::key_status();
        if (ks == "valid") {
          cfg.password = crypto::encrypt(pw, crypto::load_key());
          utils::print_info("Password from import file encrypted and stored.");
        } else {
          utils::print_warning("Key is " + ks +
                               " — password from import NOT stored.");
        }
      }
    }

    save(cfg);
    utils::print_success("Config imported from: " + path);
    logger::info("Config imported from: " + path);
    return cfg;
  } catch (const std::exception &e) {
    utils::print_error("Failed to import: " + std::string(e.what()));
    logger::error("Config import error: " + std::string(e.what()));
    return load();
  }
}

// ── set_value ────────────────────────────────────────────────────

bool set_value(Config &cfg, const std::string &key, const std::string &) {
  if (key == "password") {
    if (!cfg.remember_password) {
      utils::print_warning("remember_password is currently disabled.");
      utils::print_info(
          "To store an encrypted password, it must be enabled first.");
      std::cout << std::endl;
      std::cout << "  Enable remember_password and set a password now? [Y/n]: ";
      std::string ans;
      std::getline(std::cin, ans);
      ans = utils::trim(ans);
      if (!ans.empty() && ans[0] != 'y' && ans[0] != 'Y') {
        utils::print_info(
            "Aborted. Password will continue to be prompted at connect time.");
        return false;
      }
      cfg.remember_password = true;
      utils::print_success("remember_password enabled.");
    }
    std::string ks = crypto::key_status();
    if (ks != "valid") {
      utils::print_error("Encryption key is " + ks + "!");
      utils::print_info("Run 'exv config key reset' to fix this.");
      return false;
    }
    utils::print_info("Password input is hidden and will not be displayed.");
    std::string pw = crypto::read_password_hidden("  New password: ");
    if (pw.empty()) {
      utils::print_error("Password cannot be empty.");
      return false;
    }
    cfg.password = crypto::encrypt(pw, crypto::load_key());
    if (cfg.password.empty()) {
      utils::print_error("Encryption failed.");
      return false;
    }
    if (save(cfg)) {
      utils::print_success("Password set and encrypted.");
      logger::info("Password updated (encrypted)");
      return true;
    }
    return false;
  }

  if (key == "remember_password") {
    std::cout << "  Remember password? [Y/n]: ";
    std::string input;
    std::getline(std::cin, input);
    input = utils::trim(input);
    cfg.remember_password =
        (input.empty() || input[0] == 'y' || input[0] == 'Y');
    if (!cfg.remember_password)
      cfg.password = "";
    if (save(cfg)) {
      utils::print_success(std::string("remember_password = ") +
                           (cfg.remember_password ? "true" : "false"));
      return true;
    }
    return false;
  }

  auto handle_str = [&](const std::string &k, std::string &field) -> bool {
    if (key != k)
      return false;
    std::cout << "  Enter value for " << k << ": ";
    std::string val;
    std::getline(std::cin, val);
    val = utils::trim(val);
    if (val.empty()) {
      utils::print_error("Value cannot be empty.");
      return false;
    }
    field = val;
    if (save(cfg)) {
      utils::print_success("Set " + k + " = " + val);
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

  if (key == "mtu") {
    std::cout << "  Enter value for mtu: ";
    std::string val;
    std::getline(std::cin, val);
    try {
      cfg.mtu = std::stoi(utils::trim(val));
    } catch (...) {
      utils::print_error("Invalid MTU.");
      return false;
    }
    if (save(cfg)) {
      utils::print_success("Set mtu = " + val);
      return true;
    }
    return false;
  }

  utils::print_error("Unknown config key: " + key);
  utils::print_info("Valid keys: server, username, password, mtu, useragent, "
                    "log_file, remember_password");
  return false;
}

// ── Reset ───────────────────────────────────────────────────────

Config reset() {
  Config cfg;
  save(cfg);
  tunnel::write_script(cfg);
  utils::print_success("Config reset to defaults. Key file preserved.");
  utils::print_info("Run 'exv config set password' to set a new password.");
  logger::info("Config reset to defaults");
  return cfg;
}

// ── Route management ────────────────────────────────────────────

bool add_route(Config &cfg, const std::string &route) {
  if (std::find(cfg.routes.begin(), cfg.routes.end(), route) !=
      cfg.routes.end()) {
    utils::print_warning("Route already exists: " + route);
    return false;
  }
  cfg.routes.push_back(route);
  save(cfg);
  utils::print_success("Route added: " + route);
  logger::info("Route added: " + route);
  return true;
}

bool remove_route(Config &cfg, const std::string &route) {
  auto it = std::find(cfg.routes.begin(), cfg.routes.end(), route);
  if (it == cfg.routes.end()) {
    utils::print_error("Route not found: " + route);
    return false;
  }
  cfg.routes.erase(it);
  save(cfg);
  utils::print_success("Route removed: " + route);
  logger::info("Route removed: " + route);
  return true;
}

void list_routes(const Config &cfg) {
  utils::print_header("VPN Routes");
  if (cfg.routes.empty()) {
    utils::print_warning("No routes configured.");
    return;
  }
  std::cout << "  Total: " << cfg.routes.size() << " routes" << std::endl
            << std::endl;
  for (size_t i = 0; i < cfg.routes.size(); ++i)
    std::cout << "  " << utils::GREEN << (i + 1) << "." << utils::RESET << " "
              << cfg.routes[i] << std::endl;
  std::cout << std::endl;
}

// ── Key management ──────────────────────────────────────────────

void key_show() {
  utils::print_header("Encryption Key Status");
  std::string ks = crypto::key_status();
  std::cout << "  Key file : " << crypto::key_path() << std::endl;
  std::cout << "  Status   : ";
  if (ks == "valid")
    std::cout << utils::GREEN << utils::BOLD << "valid" << utils::RESET
              << std::endl;
  else if (ks == "missing") {
    std::cout << utils::YELLOW << utils::BOLD << "missing" << utils::RESET
              << std::endl;
    utils::print_info("Run: exv config key reset");
  } else {
    std::cout << utils::RED << utils::BOLD << "corrupt" << utils::RESET
              << std::endl;
    utils::print_warning("Run: exv config key reset");
  }
  std::cout << std::endl;
}

bool key_reset() { return crypto::reset_key(); }

} // namespace config
} // namespace ecnuvpn
