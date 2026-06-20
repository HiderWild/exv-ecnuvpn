#pragma once

namespace exv {
namespace platform {

bool is_process_alive(int pid);
bool terminate_process(int pid, bool force);
void sleep_ms(unsigned int milliseconds);

} // namespace platform
} // namespace exv
