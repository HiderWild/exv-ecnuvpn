#pragma once

namespace ecnuvpn {
namespace platform {

bool is_process_alive(int pid);
int find_openconnect_pid();
bool terminate_process(int pid, bool force);
void sleep_ms(unsigned int milliseconds);

} // namespace platform
} // namespace ecnuvpn