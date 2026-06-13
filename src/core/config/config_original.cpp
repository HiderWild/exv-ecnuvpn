#include "core/config/config.hpp"
#include "core/crypto/crypto.hpp"
#include "core/logging/logger.hpp"
#include "tunnel.hpp"
#include "utils.hpp"

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

#include "core/config/config_wizard_ui.inc.cpp"
#include "core/config/config_wizard_routes.inc.cpp"
#include "core/config/config_wizard_flow.inc.cpp"
#include "core/config/config_persistence_legacy.inc.cpp"
#include "core/config/config_show_legacy.inc.cpp"
#include "core/config/config_import_legacy.inc.cpp"
#include "core/config/config_set_value_legacy.inc.cpp"
#include "core/config/config_maintenance_legacy.inc.cpp"

} // namespace config
} // namespace ecnuvpn
