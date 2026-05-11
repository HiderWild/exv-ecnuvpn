cmake_minimum_required(VERSION 3.15)
project(exv VERSION 3.3.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# Header-only nlohmann/json
include_directories(${CMAKE_SOURCE_DIR}/include)

# Source files
set(SOURCES
    src/main.cpp
    src/config.cpp
    src/config_manager.cpp
    src/config_api.cpp
    src/crypto.cpp
    src/helper.cpp
    src/vpn.cpp
    src/tunnel.cpp
    src/logger.cpp
    src/utils.cpp
    src/sse_broadcaster.cpp
    src/webui.cpp
)

add_executable(exv ${SOURCES})

# Link pthread (required by cpp-httplib)
find_package(Threads REQUIRED)
target_link_libraries(exv PRIVATE Threads::Threads)

# Platform-specific dependencies
if(APPLE)
    # CommonCrypto links implicitly on macOS
    target_sources(exv PRIVATE src/helper_daemon_mac.cpp)
elseif(UNIX AND NOT APPLE)
    find_package(OpenSSL 1.1.1 REQUIRED)
    target_link_libraries(exv PRIVATE OpenSSL::SSL OpenSSL::Crypto)
    target_sources(exv PRIVATE src/helper_daemon_linux.cpp)
elseif(WIN32)
    find_package(OpenSSL 1.1.1 REQUIRED)
    target_link_libraries(exv PRIVATE OpenSSL::SSL OpenSSL::Crypto)
    # Windows Service API and Named Pipe
    target_link_libraries(exv PRIVATE advapi32 iphlpapi)
    target_sources(exv PRIVATE src/helper_daemon_win.cpp)
else()
    message(FATAL_ERROR "Unsupported platform. Supported: macOS, Linux, Windows.")
endif()

# Pass version to source
target_compile_definitions(exv PRIVATE
    ECNUVPN_VERSION="${PROJECT_VERSION}"
)

# Embedded WebUI assets
add_custom_command(
    OUTPUT ${CMAKE_SOURCE_DIR}/src/webui_assets.hpp
    COMMAND python3 ${CMAKE_SOURCE_DIR}/scripts/embed_assets.py
    DEPENDS ${CMAKE_SOURCE_DIR}/scripts/embed_assets.py
    WORKING_DIRECTORY ${CMAKE_SOURCE_DIR}
    COMMENT "Embedding webui assets into C++ header..."
)

add_custom_target(embed_assets DEPENDS ${CMAKE_SOURCE_DIR}/src/webui_assets.hpp)
add_dependencies(exv embed_assets)

target_compile_definitions(exv PRIVATE EMBEDDED_ASSETS)

# Install target
if(WIN32)
  install(TARGETS exv DESTINATION "C:/Program Files/ECNU-VPN")
else()
  install(TARGETS exv DESTINATION /usr/local/bin)
endif()
