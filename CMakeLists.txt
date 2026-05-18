cmake_minimum_required(VERSION 3.15)
project(exv VERSION 3.3.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

enable_testing()

# Release builds optimise for size and strip debug symbols. The MinGW
# toolchain we ship on Windows links against the gcc/g++ runtime DLLs, so
# stripping the binary itself gives the bulk of the savings (~3-4MB).
if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release CACHE STRING "" FORCE)
endif()

if(CMAKE_BUILD_TYPE STREQUAL "Release")
    if(MSVC)
        add_compile_options(/O2 /GL)
        add_link_options(/LTCG /OPT:REF /OPT:ICF)
    else()
        add_compile_options(-Os -ffunction-sections -fdata-sections)
        if(WIN32)
            add_link_options(-Wl,--gc-sections -Wl,-s)
        elseif(APPLE)
            add_link_options(-Wl,-dead_strip)
        else()
            add_link_options(-Wl,--gc-sections -Wl,-s)
        endif()
    endif()
endif()

# Header-only nlohmann/json
include_directories(${CMAKE_SOURCE_DIR}/include)

set(EXV_COMMON_SOURCES
    src/config.cpp
    src/config_manager.cpp
    src/config_api.cpp
    src/crypto.cpp
    src/helper.cpp
    src/vpn.cpp
    src/vpn_runtime.cpp
    src/tunnel.cpp
    src/virtual_network.cpp
    src/logger.cpp
    src/utils.cpp
)

# Source files
set(SOURCES
    src/main.cpp
    src/app_api.cpp
    src/sse_broadcaster.cpp
    src/webui.cpp
    ${EXV_COMMON_SOURCES}
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
    # AES-256-CBC and CSPRNG come from Windows CNG (bcrypt) so we no longer
    # need OpenSSL on this platform. The ~7MB libssl/libcrypto DLLs can be
    # dropped from the desktop bundle entirely.
    target_link_libraries(exv PRIVATE bcrypt advapi32 iphlpapi ws2_32)
    target_sources(exv PRIVATE src/helper_daemon_win.cpp)
    add_executable(exv-helper
        src/helper_service_win.cpp
        src/helper_daemon_win.cpp
        ${EXV_COMMON_SOURCES}
    )
    target_link_libraries(exv-helper PRIVATE Threads::Threads bcrypt advapi32 iphlpapi ws2_32)
    target_compile_definitions(exv-helper PRIVATE
        ECNUVPN_VERSION="${PROJECT_VERSION}"
    )
else()
    message(FATAL_ERROR "Unsupported platform. Supported: macOS, Linux, Windows.")
endif()

# Pass version to source
target_compile_definitions(exv PRIVATE
    ECNUVPN_VERSION="${PROJECT_VERSION}"
)

# Embedded WebUI assets
#
# Build dependency chain: frontend must be built before the C++ build can
# succeed.  The custom command below runs scripts/embed_assets.py, which reads
# webui/dist/ (the Vite build output) and generates src/webui_assets.hpp.
# If webui/dist/ does not exist, embed_assets.py will fail with instructions
# to build the frontend first:
#
#   cd webui && npm install && npm run build
#
# Then re-run the C++ build.  The full ordering is:
#   1. Frontend build (webui/dist/)
#   2. Native build    (this step — consumes webui/dist/)
#   3. Desktop build   (optional — packages native binary + Electron shell)
#
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

add_executable(vpn_runtime_test
        tests/vpn_runtime_test.cpp
    src/vpn_runtime.cpp
        src/utils.cpp
)

target_include_directories(vpn_runtime_test PRIVATE
        ${CMAKE_SOURCE_DIR}/include
        ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME vpn_runtime_test COMMAND vpn_runtime_test)

# Install target
if(WIN32)
  install(TARGETS exv exv-helper DESTINATION "C:/Program Files/ECNU-VPN")
else()
  install(TARGETS exv DESTINATION /usr/local/bin)
endif()
