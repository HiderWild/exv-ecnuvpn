// Core process lifecycle tests (Task E2)
//
// E2.1: Start/exit lifecycle — runs core_process_main() in a thread with
//        redirected stdin/stdout, feeds JSON-RPC requests, and verifies
//        responses and clean exit codes.
// E2.2: Crash recovery — feeds malformed / mixed input and verifies the
//        process survives without crashing and still handles valid requests.
//
// NOTE: E2.1d (SIGTERM test) MUST run last because it sets the process-global
//       g_stop_requested flag which is never reset.  All subsequent calls to
//       core_process_main() in the same process would exit immediately.

#include "core/core_process.hpp"
#include "core/tunnel_controller.hpp"
#include "core/reconnect_policy.hpp"

#include <nlohmann/json.hpp>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <csignal>
#include <cstring>
#include <iostream>
#include <memory>
#include <mutex>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

#ifdef _WIN32
#include <direct.h>
#include <windows.h>
#else
#include <sys/stat.h>
#endif

using json = nlohmann::json;

#ifndef ECNUVPN_VERSION
#define ECNUVPN_VERSION "test"
#endif

static int g_failures = 0;

static bool expect(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "EXPECT FAILED: " << message << std::endl;
        g_failures++;
        return false;
    }
    return true;
}

// ---------------------------------------------------------------------------
// Thread-safe blocking input stream buffer (simulates stdin)
// ---------------------------------------------------------------------------

class BlockingInputBuf : public std::streambuf {
    std::mutex mtx_;
    std::condition_variable cv_;
    std::string pending_;
    bool closed_ = false;

    static constexpr size_t BUF_SZ = 4096;
    char get_area_[BUF_SZ];

protected:
    int_type underflow() override {
        std::unique_lock<std::mutex> lk(mtx_);
        cv_.wait(lk, [this] { return !pending_.empty() || closed_; });

        if (pending_.empty() && closed_) {
            return traits_type::eof();
        }

        size_t n = std::min(pending_.size(), BUF_SZ);
        std::memcpy(get_area_, pending_.data(), n);
        pending_.erase(0, n);

        setg(get_area_, get_area_, get_area_ + n);
        return traits_type::to_int_type(*gptr());
    }

public:
    void feed(const std::string& data) {
        {
            std::lock_guard<std::mutex> lk(mtx_);
            pending_ += data;
        }
        cv_.notify_all();
    }

    void close_input() {
        {
            std::lock_guard<std::mutex> lk(mtx_);
            closed_ = true;
        }
        cv_.notify_all();
    }
};

// ---------------------------------------------------------------------------
// Thread-safe output capture buffer (captures stdout)
// ---------------------------------------------------------------------------

class CaptureOutputBuf : public std::streambuf {
    mutable std::mutex mtx_;
    std::condition_variable cv_;
    std::string captured_;

protected:
    int_type overflow(int_type ch) override {
        if (ch != traits_type::eof()) {
            std::lock_guard<std::mutex> lk(mtx_);
            captured_ += static_cast<char>(ch);
            cv_.notify_all();
        }
        return ch;
    }

    std::streamsize xsputn(const char* s, std::streamsize n) override {
        std::lock_guard<std::mutex> lk(mtx_);
        captured_.append(s, static_cast<size_t>(n));
        cv_.notify_all();
        return n;
    }

    int sync() override {
        return 0; // always report success to avoid badbit from tie flushes
    }

public:
    // Non-destructive read (for polling loops)
    std::string get_all() const {
        std::lock_guard<std::mutex> lk(mtx_);
        return captured_;
    }

    // Destructive read (moves data out)
    std::string read_all() {
        std::lock_guard<std::mutex> lk(mtx_);
        std::string r = std::move(captured_);
        captured_.clear();
        return r;
    }

    bool wait_for_data(std::chrono::milliseconds timeout) {
        std::unique_lock<std::mutex> lk(mtx_);
        return cv_.wait_for(lk, timeout, [this] { return !captured_.empty(); });
    }
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

static std::string make_temp_dir() {
#ifdef _WIN32
    char tmp[MAX_PATH];
    GetTempPathA(MAX_PATH, tmp);
    auto ts = static_cast<long long>(
        std::chrono::steady_clock::now().time_since_epoch().count());
    char dir[MAX_PATH];
    sprintf_s(dir, "%s\\exv_lc_%lld", tmp, ts);
    _mkdir(dir);
    return std::string(dir);
#else
    std::string tmpl = "/tmp/exv_lc_XXXXXX";
    std::vector<char> buf(tmpl.begin(), tmpl.end());
    buf.push_back('\0');
    if (char* r = mkdtemp(buf.data())) return std::string(r);
    return "/tmp/exv_lc_fallback";
#endif
}

static std::vector<json> parse_json_lines(const std::string& output) {
    std::vector<json> results;
    std::istringstream iss(output);
    std::string line;
    while (std::getline(iss, line)) {
        if (line.empty() || line.find_first_not_of(" \t\r\n") == std::string::npos)
            continue;
        try {
            results.push_back(json::parse(line));
        } catch (...) {
            // skip non-JSON lines (logger output, etc.)
        }
    }
    return results;
}

static bool wait_for_response_count(CaptureOutputBuf& buf, size_t min_count,
                                     std::chrono::milliseconds timeout) {
    auto deadline = std::chrono::steady_clock::now() + timeout;
    while (std::chrono::steady_clock::now() < deadline) {
        auto parsed = parse_json_lines(buf.get_all());
        if (parsed.size() >= min_count) return true;
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
    return false;
}

static json find_by_id(const std::vector<json>& responses, int id) {
    for (auto& r : responses) {
        if (r.value("id", -1) == id) return r;
    }
    return json();
}

// ---------------------------------------------------------------------------
// Scoped rdbuf redirector — restores original rdbuf on destruction
// ---------------------------------------------------------------------------

class ScopedRdbuf {
    std::ios& stream_;
    std::streambuf* orig_;
public:
    ScopedRdbuf(std::ios& s, std::streambuf* new_buf)
        : stream_(s), orig_(s.rdbuf()) {
        stream_.rdbuf(new_buf);
    }
    ~ScopedRdbuf() { stream_.rdbuf(orig_); }
    ScopedRdbuf(const ScopedRdbuf&) = delete;
    ScopedRdbuf& operator=(const ScopedRdbuf&) = delete;
};

// ---------------------------------------------------------------------------
// Run core_process_main in a thread and return {thread, exit_code_ref}
// ---------------------------------------------------------------------------

struct CoreRunner {
    std::thread thread;
    std::atomic<int> rc{-1};

    void start(const std::string& config_dir, const std::string& home_dir) {
        thread = std::thread([this, config_dir, home_dir] {
            rc = exv::core::core_process_main(config_dir, home_dir);
        });
    }

    void join() {
        if (thread.joinable()) thread.join();
    }
};

// ===========================================================================
// Tests — order matters: E2.1d (SIGTERM) MUST be last because it sets the
//         process-global g_stop_requested flag which is never reset.
// ===========================================================================

int main() {
    std::string config_dir = make_temp_dir();
    std::string home_dir   = make_temp_dir();
    std::cerr << "config_dir=" << config_dir << "  home_dir=" << home_dir << "\n";

    // =======================================================================
    // E2.1a — Start and respond to status.get
    // =======================================================================
    {
        std::cerr << "[E2.1a] start + status.get\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        in_buf.feed(R"({"id":1,"action":"status.get","payload":{}})" "\n");
        expect(out_buf.wait_for_data(std::chrono::seconds(3)),
               "E2.1a: should receive response");

        in_buf.close_input();
        cr.join();

        auto resp = find_by_id(parse_json_lines(out_buf.read_all()), 1);
        expect(!resp.is_null(),          "E2.1a: response exists");
        expect(resp.value("ok", false),  "E2.1a: ok=true");
        if (resp.contains("data")) {
            expect(resp["data"].value("phase", "") == "idle",
                   "E2.1a: phase=idle");
        }
        expect(cr.rc == 0, "E2.1a: exit code 0");
    }

    // =======================================================================
    // E2.1b — Multiple sequential requests
    // =======================================================================
    {
        std::cerr << "[E2.1b] multiple sequential requests\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        in_buf.feed(R"({"id":1,"action":"status.get","payload":{}})"          "\n");
        in_buf.feed(R"({"id":2,"action":"runtime.status","payload":{}})"       "\n");
        in_buf.feed(R"({"id":3,"action":"nonexistent.action","payload":{}})"   "\n");

        expect(wait_for_response_count(out_buf, 3, std::chrono::seconds(5)),
               "E2.1b: should get 3 responses");

        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());
        auto r1 = find_by_id(all, 1);
        auto r2 = find_by_id(all, 2);
        auto r3 = find_by_id(all, 3);

        expect(!r1.is_null() && r1.value("ok", false),
               "E2.1b: status.get ok");
        expect(!r2.is_null() && r2.value("ok", false),
               "E2.1b: runtime.status ok");
        expect(!r3.is_null() && !r3.value("ok", true),
               "E2.1b: unknown action fails");
        expect(cr.rc == 0, "E2.1b: exit code 0");
    }

    // =======================================================================
    // E2.1c — Graceful shutdown on stdin EOF
    // =======================================================================
    {
        std::cerr << "[E2.1c] graceful shutdown on EOF\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        // Let process start (it blocks on getline), then close stdin immediately
        std::this_thread::sleep_for(std::chrono::milliseconds(200));
        in_buf.close_input();
        cr.join();

        expect(cr.rc == 0, "E2.1c: exit 0 on EOF");
    }

    // =======================================================================
    // E2.2a — Malformed JSON input (crash recovery)
    // =======================================================================
    {
        std::cerr << "[E2.2a] malformed JSON\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        // Feed malformed + valid input upfront to avoid timing issues
        in_buf.feed("this is definitely not json\n");
        in_buf.feed(R"({"id":10,"action":"status.get","payload":{}})" "\n");

        // Wait for both responses (parse_error + valid)
        expect(wait_for_response_count(out_buf, 2, std::chrono::seconds(5)),
               "E2.2a: parse_error + valid response");

        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());

        // First response should be a parse_error (id=0)
        auto bad_resp = all.empty() ? json() : all[0];
        if (!bad_resp.is_null()) {
            expect(bad_resp.value("ok", true) == false,
                   "E2.2a: bad input returns ok=false");
            expect(bad_resp.value("code", "") == "parse_error",
                   "E2.2a: error code is parse_error");
        }

        // Second response should be valid (id=10)
        auto good_resp = find_by_id(all, 10);
        expect(!good_resp.is_null() && good_resp.value("ok", false),
               "E2.2a: valid request succeeds after bad input");

        expect(cr.rc == 0, "E2.2a: exit 0 (process survived)");
    }

    // =======================================================================
    // E2.2b — Missing action field
    // =======================================================================
    {
        std::cerr << "[E2.2b] missing action field\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        // Feed all input upfront
        in_buf.feed(R"({"id":1,"payload":{}})" "\n");
        in_buf.feed(R"({"id":2,"action":"status.get","payload":{}})" "\n");

        // Wait for both responses (missing_action + valid)
        expect(wait_for_response_count(out_buf, 2, std::chrono::seconds(5)),
               "E2.2b: missing_action + valid response");

        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());

        // First response: missing_action error
        auto missing_resp = all.empty() ? json() : all[0];
        if (!missing_resp.is_null()) {
            expect(missing_resp.value("ok", true) == false,
                   "E2.2b: missing_action returns ok=false");
            expect(missing_resp.value("code", "") == "missing_action",
                   "E2.2b: code=missing_action");
        }

        // Second response: valid status.get
        auto valid_resp = find_by_id(all, 2);
        expect(!valid_resp.is_null() && valid_resp.value("ok", false),
               "E2.2b: valid request succeeds after missing action");

        expect(cr.rc == 0, "E2.2b: exit 0");
    }

    // =======================================================================
    // E2.2c — Mixed bad + good input (crash recovery sequence)
    // =======================================================================
    {
        std::cerr << "[E2.2c] mixed bad+good input burst\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        // Burst: empty line, malformed JSON, valid requests
        in_buf.feed("\n");
        in_buf.feed("{bad json!!\n");
        in_buf.feed(R"({"id":20,"action":"status.get","payload":{}})" "\n");
        in_buf.feed(R"({"id":21,"action":"vpn.set_auto_reconnect","payload":{"enabled":false}})" "\n");

        expect(wait_for_response_count(out_buf, 3, std::chrono::seconds(5)),
               "E2.2c: got responses for parse_error + 2 valid requests");

        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());
        auto r20 = find_by_id(all, 20);
        auto r21 = find_by_id(all, 21);

        expect(!r20.is_null() && r20.value("ok", false),
               "E2.2c: status.get ok after burst");
        expect(!r21.is_null() && r21.value("ok", false),
               "E2.2c: set_auto_reconnect ok after burst");
        expect(cr.rc == 0, "E2.2c: exit 0");
    }

    // =======================================================================
    // E2.1d — Graceful shutdown on SIGTERM
    // MUST BE LAST: sets process-global g_stop_requested, never reset.
    // =======================================================================
    {
        std::cerr << "[E2.1d] graceful shutdown on SIGTERM\n";
        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(config_dir, home_dir);

        // Feed a request and wait for response
        in_buf.feed(R"({"id":1,"action":"status.get","payload":{}})" "\n");
        out_buf.wait_for_data(std::chrono::seconds(3));

        // Process is now blocking on getline. Deliver SIGTERM.
        std::raise(SIGTERM);

        // Unblock getline so the loop can check g_stop_requested
        in_buf.close_input();
        cr.join();

        expect(cr.rc == 0, "E2.1d: exit 0 on SIGTERM");
    }

    // =======================================================================
    // Cleanup
    // =======================================================================
#ifdef _WIN32
    RemoveDirectoryA(config_dir.c_str());
    RemoveDirectoryA(home_dir.c_str());
#else
    rmdir(config_dir.c_str());
    rmdir(home_dir.c_str());
#endif

    if (g_failures == 0) {
        std::cout << "core_process_lifecycle_test: all assertions passed\n";
        return 0;
    } else {
        std::cerr << "core_process_lifecycle_test: " << g_failures
                  << " assertion(s) FAILED\n";
        return 1;
    }
}
