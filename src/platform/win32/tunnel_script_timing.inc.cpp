class TunnelTiming {
public:
  TunnelTiming() : started_(Clock::now()), last_(started_) {
    logger::info("[connect-timing] scope=tunnel.windows stage=begin delta_ms=0 total_ms=0");
  }

  void mark(const std::string &stage, const std::string &detail = "") {
    auto now = Clock::now();
    auto delta_ms = elapsed_ms(last_, now);
    auto total_ms = elapsed_ms(started_, now);
    last_ = now;
    std::string message = "[connect-timing] scope=tunnel.windows stage=" +
                          stage + " delta_ms=" + std::to_string(delta_ms) +
                          " total_ms=" + std::to_string(total_ms);
    if (!detail.empty())
      message += " " + detail;
    logger::info(message);
  }

  void finish(bool ok, const std::string &detail = "") {
    if (finished_)
      return;
    finished_ = true;
    mark(ok ? "finish.ok" : "finish.error", detail);
  }

private:
  using Clock = std::chrono::steady_clock;

  static long long elapsed_ms(const Clock::time_point &from,
                              const Clock::time_point &to) {
    return std::chrono::duration_cast<std::chrono::milliseconds>(to - from)
        .count();
  }

  Clock::time_point started_;
  Clock::time_point last_;
  bool finished_ = false;
};

