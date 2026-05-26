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
include_directories(${CMAKE_SOURCE_DIR}/src)

set(EXV_COMMON_SOURCES
    src/config.cpp
    src/config_manager.cpp
    src/config_api.cpp
    src/crypto.cpp
    src/platform/common/backend_resolver.cpp
    src/platform/common/oneshot_bootstrap.cpp
    src/platform/common/proxy_tun_detector.cpp
    src/vpn.cpp
    src/vpn_runtime.cpp
    src/tunnel.cpp
    src/virtual_network.cpp
    src/logger.cpp
    src/utils.cpp
)

set(EXV_HELPER_RUNTIME_SOURCES
    src/helper.cpp
)

set(EXV_PLATFORM_SOURCES)
set(EXV_PLATFORM_LIBS)

# Source files
set(SOURCES
    src/main.cpp
    src/app_api.cpp
    src/sse_broadcaster.cpp
    src/webui.cpp
    src/platform/common/status_models.cpp
    src/platform/common/runtime_status.cpp
)
find_package(Threads REQUIRED)

# Platform-specific dependencies
if(APPLE)
    # CommonCrypto links implicitly on macOS
    list(APPEND EXV_PLATFORM_SOURCES
        src/helper_daemon_mac.cpp
        src/platform/darwin/app_api_runtime_policy.cpp
        src/platform/darwin/crypto_backend.cpp
        src/platform/darwin/config_defaults.cpp
        src/platform/darwin/helper_client.cpp
        src/platform/darwin/helper_lifecycle.cpp
        src/platform/darwin/helper_platform.cpp
        src/platform/darwin/helper_service_manager.cpp
        src/platform/darwin/openconnect_process.cpp
        src/platform/darwin/oneshot_bootstrap.cpp
        src/platform/darwin/process_control.cpp
        src/platform/darwin/tunnel_script.cpp
        src/platform/darwin/virtual_network_probe.cpp
        src/platform/darwin/vpn_supervisor_process.cpp
        src/platform/darwin/path_utils.cpp
        src/platform/darwin/service_status.cpp
        src/platform/common/driver_status_stub.cpp
    )
elseif(UNIX AND NOT APPLE)
    find_package(OpenSSL 1.1.1 REQUIRED)
    list(APPEND EXV_PLATFORM_LIBS OpenSSL::SSL OpenSSL::Crypto)
    list(APPEND EXV_PLATFORM_SOURCES
        src/helper_daemon_linux.cpp
        src/platform/linux/app_api_runtime_policy.cpp
        src/platform/linux/crypto_backend.cpp
        src/platform/common/config_defaults_linux.cpp
        src/platform/linux/helper_client.cpp
        src/platform/linux/helper_lifecycle.cpp
        src/platform/linux/helper_platform.cpp
        src/platform/linux/helper_service_manager.cpp
        src/platform/linux/openconnect_process.cpp
        src/platform/linux/oneshot_bootstrap.cpp
        src/platform/linux/process_control.cpp
        src/platform/linux/tunnel_script.cpp
        src/platform/linux/virtual_network_probe.cpp
        src/platform/linux/vpn_supervisor_process.cpp
        src/platform/linux/path_utils.cpp
        src/platform/common/service_status_linux.cpp
        src/platform/common/driver_status_stub.cpp
    )
elseif(WIN32)
    # AES-256-CBC and CSPRNG come from Windows CNG (bcrypt) so we no longer
    # need OpenSSL on this platform. The ~7MB libssl/libcrypto DLLs can be
    # dropped from the desktop bundle entirely.
    list(APPEND EXV_PLATFORM_LIBS bcrypt advapi32 iphlpapi ws2_32)
    list(APPEND EXV_PLATFORM_SOURCES
        src/helper_daemon_win.cpp
        src/platform/win32/app_api_runtime_policy.cpp
        src/platform/win32/crypto_backend.cpp
        src/platform/win32/config_defaults.cpp
        src/platform/win32/helper_client.cpp
        src/platform/win32/helper_lifecycle.cpp
        src/platform/win32/helper_platform.cpp
        src/platform/win32/helper_service_manager.cpp
        src/platform/win32/openconnect_process.cpp
        src/platform/win32/oneshot_bootstrap.cpp
        src/platform/win32/process_control.cpp
        src/platform/win32/tunnel_script.cpp
        src/platform/win32/virtual_network_probe.cpp
        src/platform/win32/vpn_supervisor_process.cpp
        src/platform/win32/path_utils.cpp
        src/platform/win32/service_status.cpp
        src/platform/win32/driver_status.cpp
    )
else()
    message(FATAL_ERROR "Unsupported platform. Supported: macOS, Linux, Windows.")
endif()

# This is the first native split toward the documented architecture.  The
# platform adapter files still live in this target because the current core
# sources call platform functions directly; C1/C2 will split those adapters
# behind cleaner interfaces.
add_library(exv-core STATIC
    ${EXV_COMMON_SOURCES}
    ${EXV_PLATFORM_SOURCES}
)
target_include_directories(exv-core PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
target_link_libraries(exv-core PUBLIC Threads::Threads ${EXV_PLATFORM_LIBS})
target_compile_definitions(exv-core PUBLIC
    ECNUVPN_VERSION="${PROJECT_VERSION}"
)

add_library(exv-helper-runtime STATIC
    ${EXV_HELPER_RUNTIME_SOURCES}
)
target_include_directories(exv-helper-runtime PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
target_link_libraries(exv-helper-runtime PUBLIC exv-core)
target_compile_definitions(exv-helper-runtime PUBLIC
    ECNUVPN_VERSION="${PROJECT_VERSION}"
)

add_executable(exv ${SOURCES})
target_link_libraries(exv PRIVATE exv-helper-runtime)

add_executable(exv-helper
    src/helper_main.cpp
)
target_link_libraries(exv-helper PRIVATE exv-helper-runtime)
target_compile_definitions(exv-helper PRIVATE
    ECNUVPN_VERSION="${PROJECT_VERSION}"
)

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

if(APPLE)
    target_sources(vpn_runtime_test PRIVATE src/platform/darwin/path_utils.cpp src/platform/darwin/process_control.cpp)
elseif(UNIX AND NOT APPLE)
    target_sources(vpn_runtime_test PRIVATE src/platform/linux/path_utils.cpp src/platform/linux/process_control.cpp)
elseif(WIN32)
    target_sources(vpn_runtime_test PRIVATE src/platform/win32/path_utils.cpp src/platform/win32/process_control.cpp)
endif()

target_include_directories(vpn_runtime_test PRIVATE
        ${CMAKE_SOURCE_DIR}/include
        ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME vpn_runtime_test COMMAND vpn_runtime_test)

add_executable(platform_status_models_test
    tests/platform_status_models_test.cpp
    src/platform/common/status_models.cpp
)

target_include_directories(platform_status_models_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME platform_status_models_test COMMAND platform_status_models_test)

add_executable(backend_resolver_test
    tests/backend_resolver_test.cpp
    src/platform/common/backend_resolver.cpp
    src/platform/common/oneshot_bootstrap.cpp
    src/platform/common/status_models.cpp
)

target_include_directories(backend_resolver_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME backend_resolver_test COMMAND backend_resolver_test)

add_executable(tunnel_script_contract_test
    tests/tunnel_script_contract_test.cpp
    src/tunnel.cpp
)

if(APPLE)
    target_sources(tunnel_script_contract_test PRIVATE src/platform/darwin/tunnel_script.cpp)
elseif(UNIX AND NOT APPLE)
    target_sources(tunnel_script_contract_test PRIVATE src/platform/linux/tunnel_script.cpp)
elseif(WIN32)
    target_sources(tunnel_script_contract_test PRIVATE src/platform/win32/tunnel_script.cpp)
    target_link_libraries(tunnel_script_contract_test PRIVATE ws2_32)
endif()

target_include_directories(tunnel_script_contract_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME tunnel_script_contract_test COMMAND tunnel_script_contract_test)

add_executable(proxy_tun_detector_test
    tests/proxy_tun_detector_test.cpp
    src/platform/common/proxy_tun_detector.cpp
)

target_include_directories(proxy_tun_detector_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME proxy_tun_detector_test COMMAND proxy_tun_detector_test)

add_executable(app_api_runtime_policy_test
    tests/app_api_runtime_policy_test.cpp
)

if(APPLE)
    target_sources(app_api_runtime_policy_test PRIVATE src/platform/darwin/app_api_runtime_policy.cpp)
elseif(UNIX AND NOT APPLE)
    target_sources(app_api_runtime_policy_test PRIVATE src/platform/linux/app_api_runtime_policy.cpp)
elseif(WIN32)
    target_sources(app_api_runtime_policy_test PRIVATE src/platform/win32/app_api_runtime_policy.cpp)
endif()

target_include_directories(app_api_runtime_policy_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME app_api_runtime_policy_test COMMAND app_api_runtime_policy_test)

add_executable(crypto_roundtrip_test
    tests/crypto_roundtrip_test.cpp
    src/crypto.cpp
)

if(APPLE)
    target_sources(crypto_roundtrip_test PRIVATE src/platform/darwin/crypto_backend.cpp)
elseif(UNIX AND NOT APPLE)
    target_sources(crypto_roundtrip_test PRIVATE src/platform/linux/crypto_backend.cpp)
    target_link_libraries(crypto_roundtrip_test PRIVATE OpenSSL::SSL OpenSSL::Crypto)
elseif(WIN32)
    target_sources(crypto_roundtrip_test PRIVATE src/platform/win32/crypto_backend.cpp)
    target_link_libraries(crypto_roundtrip_test PRIVATE bcrypt advapi32)
endif()

target_include_directories(crypto_roundtrip_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME crypto_roundtrip_test COMMAND crypto_roundtrip_test)

# Install target
if(WIN32)
  install(TARGETS exv exv-helper DESTINATION "C:/Program Files/ECNU-VPN")
else()
  install(TARGETS exv exv-helper DESTINATION /usr/local/bin)
endif()
