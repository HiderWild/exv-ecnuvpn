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

set(EXV_PROTOCOL_SOURCES
    src/vpn_engine/protocol/auth.cpp
    src/vpn_engine/protocol/cstp.cpp
    src/vpn_engine/protocol/http.cpp
    src/vpn_engine/protocol/production_transport.cpp
    src/vpn_engine/protocol/session.cpp
    src/vpn_engine/protocol/tls_stream.cpp
    src/vpn_engine/protocol/url.cpp
)

set(EXV_COMMON_SOURCES
    src/config.cpp
    src/config_manager.cpp
    src/config_api.cpp
    src/crypto.cpp
    src/platform/common/backend_resolver.cpp
    src/platform/common/oneshot_bootstrap.cpp
    src/platform/common/proxy_tun_detector.cpp
    src/openconnect_log.cpp
    src/vpn_engine/native_engine.cpp
    src/vpn_engine/native_session_store.cpp
    ${EXV_PROTOCOL_SOURCES}
    src/vpn_engine/session_state.cpp
    src/vpn.cpp
    src/vpn_runtime.cpp
    src/tunnel.cpp
    src/virtual_network.cpp
    src/runtime/runtime_context.cpp
    src/feedback/feedback.cpp
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
    list(APPEND EXV_PLATFORM_LIBS "-framework Security" "-framework CoreFoundation")
    list(APPEND EXV_PLATFORM_SOURCES
        src/helper_daemon_mac.cpp
        src/platform/darwin/app_api_runtime_policy.cpp
        src/platform/darwin/crypto_backend.cpp
        src/platform/darwin/config_defaults.cpp
        src/platform/darwin/helper_client.cpp
        src/platform/darwin/helper_lifecycle.cpp
        src/platform/darwin/helper_platform.cpp
        src/platform/darwin/helper_service_manager.cpp
        src/platform/darwin/native_tls_stream.cpp
        src/platform/darwin/native_packet_device.cpp
        src/platform/darwin/native_route_config.cpp
        src/platform/darwin/native_utun.cpp
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
    list(APPEND EXV_PLATFORM_LIBS bcrypt advapi32 iphlpapi ws2_32 secur32 crypt32)
    list(APPEND EXV_PLATFORM_SOURCES
        src/helper_daemon_win.cpp
        src/platform/win32/app_api_runtime_policy.cpp
        src/platform/win32/crypto_backend.cpp
        src/platform/win32/config_defaults.cpp
        src/platform/win32/helper_client.cpp
        src/platform/win32/helper_lifecycle.cpp
        src/platform/win32/helper_platform.cpp
        src/platform/win32/helper_service_manager.cpp
        src/platform/win32/native_tls_stream.cpp
        src/platform/win32/native_packet_device.cpp
        src/platform/win32/native_wintun.cpp
        src/platform/win32/native_ip_config.cpp
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
    src/vpn_engine/native_session_store.cpp
    src/vpn_engine/session_state.cpp
    src/vpn_runtime.cpp
        src/utils.cpp
)

if(APPLE)
    target_sources(vpn_runtime_test PRIVATE src/platform/darwin/path_utils.cpp src/platform/darwin/process_control.cpp src/platform/darwin/config_defaults.cpp)
elseif(UNIX AND NOT APPLE)
    target_sources(vpn_runtime_test PRIVATE src/platform/linux/path_utils.cpp src/platform/linux/process_control.cpp src/platform/common/config_defaults_linux.cpp)
elseif(WIN32)
    target_sources(vpn_runtime_test PRIVATE src/platform/win32/path_utils.cpp src/platform/win32/process_control.cpp src/platform/win32/config_defaults.cpp)
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

add_executable(feedback_test
    tests/feedback_test.cpp
    src/feedback/feedback.cpp
)

target_include_directories(feedback_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME feedback_test COMMAND feedback_test)

add_executable(runtime_status_native_test
    tests/runtime_status_native_test.cpp
    src/platform/common/runtime_status.cpp
    src/platform/common/status_models.cpp
)

if(APPLE)
    target_sources(runtime_status_native_test PRIVATE src/platform/darwin/config_defaults.cpp)
elseif(UNIX AND NOT APPLE)
    target_sources(runtime_status_native_test PRIVATE src/platform/common/config_defaults_linux.cpp)
elseif(WIN32)
    target_sources(runtime_status_native_test PRIVATE src/platform/win32/config_defaults.cpp)
endif()

target_include_directories(runtime_status_native_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME runtime_status_native_test COMMAND runtime_status_native_test)

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

add_executable(openconnect_log_test
    tests/openconnect_log_test.cpp
    src/openconnect_log.cpp
)

target_include_directories(openconnect_log_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME openconnect_log_test COMMAND openconnect_log_test)

add_executable(native_session_state_test
    tests/native_session_state_test.cpp
    src/vpn_engine/session_state.cpp
)

target_include_directories(native_session_state_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME native_session_state_test COMMAND native_session_state_test)

add_executable(native_helper_session_test
    tests/native_helper_session_test.cpp
    src/vpn_engine/native_session_store.cpp
    src/vpn_engine/session_state.cpp
)

target_include_directories(native_helper_session_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME native_helper_session_test COMMAND native_helper_session_test)

add_executable(native_engine_contract_test
    tests/native_engine_contract_test.cpp
    src/vpn_engine/native_engine.cpp
    src/vpn_engine/protocol/auth.cpp
    src/vpn_engine/protocol/cstp.cpp
    src/vpn_engine/protocol/http.cpp
    src/vpn_engine/protocol/production_transport.cpp
    src/vpn_engine/protocol/session.cpp
    src/vpn_engine/protocol/tls_stream.cpp
    src/vpn_engine/protocol/url.cpp
    src/vpn_engine/session_state.cpp
)

if(APPLE)
    target_sources(native_engine_contract_test PRIVATE
        src/platform/darwin/config_defaults.cpp
        src/platform/darwin/native_tls_stream.cpp
        src/platform/darwin/native_packet_device.cpp
        src/platform/darwin/native_route_config.cpp
        src/platform/darwin/native_utun.cpp
    )
elseif(UNIX AND NOT APPLE)
    target_sources(native_engine_contract_test PRIVATE src/platform/common/config_defaults_linux.cpp)
elseif(WIN32)
    target_sources(native_engine_contract_test PRIVATE
        src/platform/win32/config_defaults.cpp
        src/platform/win32/native_tls_stream.cpp
        src/platform/win32/native_packet_device.cpp
        src/platform/win32/native_wintun.cpp
        src/platform/win32/native_ip_config.cpp
        src/platform/win32/path_utils.cpp
        src/utils.cpp
    )
endif()

target_include_directories(native_engine_contract_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

target_link_libraries(native_engine_contract_test PRIVATE Threads::Threads ${EXV_PLATFORM_LIBS})

add_test(NAME native_engine_contract_test COMMAND native_engine_contract_test)

add_executable(native_event_sink_test
    tests/native_event_sink_test.cpp
    src/vpn_engine/event_sink.cpp
    src/vpn_engine/native_engine.cpp
    src/vpn_engine/protocol/auth.cpp
    src/vpn_engine/protocol/cstp.cpp
    src/vpn_engine/protocol/http.cpp
    src/vpn_engine/protocol/production_transport.cpp
    src/vpn_engine/protocol/session.cpp
    src/vpn_engine/protocol/tls_stream.cpp
    src/vpn_engine/protocol/url.cpp
    src/vpn_engine/session_state.cpp
)

if(APPLE)
    target_sources(native_event_sink_test PRIVATE
        src/platform/darwin/config_defaults.cpp
        src/platform/darwin/native_tls_stream.cpp
        src/platform/darwin/native_packet_device.cpp
        src/platform/darwin/native_route_config.cpp
        src/platform/darwin/native_utun.cpp
    )
elseif(UNIX AND NOT APPLE)
    target_sources(native_event_sink_test PRIVATE src/platform/common/config_defaults_linux.cpp)
elseif(WIN32)
    target_sources(native_event_sink_test PRIVATE
        src/platform/win32/config_defaults.cpp
        src/platform/win32/native_tls_stream.cpp
        src/platform/win32/native_packet_device.cpp
        src/platform/win32/native_wintun.cpp
        src/platform/win32/native_ip_config.cpp
        src/platform/win32/path_utils.cpp
        src/utils.cpp
    )
endif()

target_include_directories(native_event_sink_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

target_link_libraries(native_event_sink_test PRIVATE Threads::Threads ${EXV_PLATFORM_LIBS})

add_test(NAME native_event_sink_test COMMAND native_event_sink_test)

add_executable(tunnel_script_contract_test
    tests/tunnel_script_contract_test.cpp
    src/tunnel.cpp
    src/openconnect_log.cpp
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

add_executable(native_packaging_policy_test
    tests/native_packaging_policy_test.cpp
)

target_compile_definitions(native_packaging_policy_test PRIVATE
    ECNUVPN_SOURCE_DIR="${CMAKE_SOURCE_DIR}"
)

add_test(NAME native_packaging_policy_test COMMAND native_packaging_policy_test)

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

add_executable(native_url_test
    tests/native_url_test.cpp
    src/vpn_engine/protocol/url.cpp
    src/vpn_engine/protocol/http.cpp
)

target_include_directories(native_url_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME native_url_test COMMAND native_url_test)

add_executable(native_cstp_frame_test
    tests/native_cstp_frame_test.cpp
    src/vpn_engine/protocol/http.cpp
    src/vpn_engine/protocol/cstp.cpp
)

target_include_directories(native_cstp_frame_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME native_cstp_frame_test COMMAND native_cstp_frame_test)

add_executable(native_tls_stream_contract_test
    tests/native_tls_stream_contract_test.cpp
    src/vpn_engine/protocol/tls_stream.cpp
)

target_include_directories(native_tls_stream_contract_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

target_compile_definitions(native_tls_stream_contract_test PRIVATE
    ECNUVPN_SOURCE_DIR="${CMAKE_SOURCE_DIR}"
)

add_test(NAME native_tls_stream_contract_test COMMAND native_tls_stream_contract_test)

add_executable(native_auth_parser_test
    tests/native_auth_parser_test.cpp
    src/vpn_engine/protocol/http.cpp
    src/vpn_engine/protocol/auth.cpp
)

target_include_directories(native_auth_parser_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME native_auth_parser_test COMMAND native_auth_parser_test)

add_executable(native_protocol_session_test
    tests/native_protocol_session_test.cpp
    tests/support/fake_anyconnect_server.cpp
    src/vpn_engine/protocol/session.cpp
    src/vpn_engine/protocol/http.cpp
    src/vpn_engine/protocol/auth.cpp
    src/vpn_engine/protocol/cstp.cpp
    src/vpn_engine/session_state.cpp
)

target_include_directories(native_protocol_session_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
    ${CMAKE_SOURCE_DIR}/tests
)

add_test(NAME native_protocol_session_test COMMAND native_protocol_session_test)

add_executable(native_production_transport_test
    tests/native_production_transport_test.cpp
    src/vpn_engine/protocol/http.cpp
    src/vpn_engine/protocol/auth.cpp
    src/vpn_engine/protocol/cstp.cpp
    src/vpn_engine/protocol/production_transport.cpp
)

target_include_directories(native_production_transport_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
    ${CMAKE_SOURCE_DIR}/tests
)

add_test(NAME native_production_transport_test COMMAND native_production_transport_test)

add_executable(native_fake_anyconnect_server_test
    tests/native_fake_anyconnect_server_test.cpp
    tests/support/fake_anyconnect_server.cpp
    src/vpn_engine/protocol/http.cpp
    src/vpn_engine/protocol/auth.cpp
    src/vpn_engine/protocol/cstp.cpp
    src/vpn_engine/session_state.cpp
)

target_include_directories(native_fake_anyconnect_server_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
    ${CMAKE_SOURCE_DIR}/tests
)

add_test(NAME native_fake_anyconnect_server_test COMMAND native_fake_anyconnect_server_test)

if(WIN32)
    add_executable(win32_native_wintun_test
        tests/win32_native_wintun_test.cpp
        src/platform/win32/native_wintun.cpp
    )

    target_include_directories(win32_native_wintun_test PRIVATE
        ${CMAKE_SOURCE_DIR}/include
        ${CMAKE_SOURCE_DIR}/src
    )

    target_link_libraries(win32_native_wintun_test PRIVATE iphlpapi)

    add_test(NAME win32_native_wintun_test COMMAND win32_native_wintun_test)

    add_executable(win32_native_ip_config_test
        tests/win32_native_ip_config_test.cpp
        src/platform/win32/native_ip_config.cpp
    )

    target_include_directories(win32_native_ip_config_test PRIVATE
        ${CMAKE_SOURCE_DIR}/include
        ${CMAKE_SOURCE_DIR}/src
    )

    target_link_libraries(win32_native_ip_config_test PRIVATE iphlpapi ws2_32)

    add_test(NAME win32_native_ip_config_test COMMAND win32_native_ip_config_test)

    add_executable(win32_native_packet_device_test
        tests/win32_native_packet_device_test.cpp
        src/platform/win32/native_packet_device.cpp
        src/platform/win32/native_wintun.cpp
        src/platform/win32/native_ip_config.cpp
    )

    target_include_directories(win32_native_packet_device_test PRIVATE
        ${CMAKE_SOURCE_DIR}/include
        ${CMAKE_SOURCE_DIR}/src
    )

    target_link_libraries(win32_native_packet_device_test PRIVATE iphlpapi ws2_32)

    add_test(NAME win32_native_packet_device_test COMMAND win32_native_packet_device_test)

    add_executable(win32_native_tls_stream_test
        tests/win32_native_tls_stream_test.cpp
        src/platform/win32/native_tls_stream.cpp
    )

    target_include_directories(win32_native_tls_stream_test PRIVATE
        ${CMAKE_SOURCE_DIR}/include
        ${CMAKE_SOURCE_DIR}/src
    )

    target_link_libraries(win32_native_tls_stream_test PRIVATE ws2_32 secur32 crypt32)

    add_test(NAME win32_native_tls_stream_test COMMAND win32_native_tls_stream_test)
endif()

if(APPLE)
    add_executable(darwin_native_utun_test
        tests/darwin_native_utun_test.cpp
        src/platform/darwin/native_utun.cpp
    )

    target_include_directories(darwin_native_utun_test PRIVATE
        ${CMAKE_SOURCE_DIR}/include
        ${CMAKE_SOURCE_DIR}/src
    )

    add_test(NAME darwin_native_utun_test COMMAND darwin_native_utun_test)
endif()

if(APPLE)
    add_executable(darwin_native_route_config_test
        tests/darwin_native_route_config_test.cpp
        src/platform/darwin/native_route_config.cpp
    )

    target_include_directories(darwin_native_route_config_test PRIVATE
        ${CMAKE_SOURCE_DIR}/include
        ${CMAKE_SOURCE_DIR}/src
    )

    add_test(NAME darwin_native_route_config_test COMMAND darwin_native_route_config_test)

    add_executable(darwin_native_packet_device_test
        tests/darwin_native_packet_device_test.cpp
        src/platform/darwin/native_packet_device.cpp
        src/platform/darwin/native_utun.cpp
        src/platform/darwin/native_route_config.cpp
    )

    target_include_directories(darwin_native_packet_device_test PRIVATE
        ${CMAKE_SOURCE_DIR}/include
        ${CMAKE_SOURCE_DIR}/src
    )

    add_test(NAME darwin_native_packet_device_test COMMAND darwin_native_packet_device_test)

    add_executable(darwin_native_tls_stream_test
        tests/darwin_native_tls_stream_test.cpp
        src/platform/darwin/native_tls_stream.cpp
    )

    target_include_directories(darwin_native_tls_stream_test PRIVATE
        ${CMAKE_SOURCE_DIR}/include
        ${CMAKE_SOURCE_DIR}/src
    )

    target_link_libraries(darwin_native_tls_stream_test PRIVATE
        "-framework Security"
        "-framework CoreFoundation"
    )

    add_test(NAME darwin_native_tls_stream_test COMMAND darwin_native_tls_stream_test)
endif()

# Install target
if(WIN32)
  install(TARGETS exv exv-helper DESTINATION "C:/Program Files/ECNU-VPN")
else()
  install(TARGETS exv exv-helper DESTINATION /usr/local/bin)
endif()
