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
