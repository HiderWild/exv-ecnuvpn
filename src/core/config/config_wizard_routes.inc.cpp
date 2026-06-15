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
        std::string route = utils::trim(st.input);
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
