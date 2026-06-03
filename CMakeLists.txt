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
    src/core/core_process.cpp
    src/core/tunnel_controller.cpp
    src/core/timing.cpp
    src/core/core_error_mapper.cpp
    src/core/reconnect_policy.cpp
    src/core_api/app_rpc_dispatcher.cpp
    src/core_api/vpn_actions.cpp
    src/core_api/config_actions.cpp
    src/core_api/service_actions.cpp
    src/core_api/route_actions.cpp
    src/core_api/core_api_setup.cpp
    src/crypto.cpp
    src/helper_common/helper_connector.cpp
    src/helper_common/pipe_helper_client.cpp
    src/platform/common/backend_resolver.cpp
    src/platform/common/credential_store.cpp
    src/platform/common/dns_model.cpp
    src/platform/common/oneshot_bootstrap.cpp
    src/platform/common/packet_device_factory.cpp
    src/platform/common/platform_network_ops.cpp
    src/platform/common/helper_delegating_network_ops.cpp
    src/platform/common/proxy_tun_detector.cpp
    src/platform/common/route_model.cpp
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
    src/connection_attempt.cpp
    src/logger.cpp
    src/utils.cpp
)

set(EXV_HELPER_RUNTIME_SOURCES
    src/helper.cpp
    src/helper_common/helper_messages.cpp
    src/helper_runtime/helper_request_dispatcher.cpp
    src/helper_runtime/helper_server.cpp
    src/helper_v2_handler.cpp
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
        src/platform/darwin/darwin_keychain_store.cpp
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
        src/platform/win32/win_route_ops.cpp
        src/platform/win32/win_credential_store.cpp
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

add_executable(platform_network_ops_model_test
    tests/platform_network_ops_model_test.cpp
    src/platform/common/route_model.cpp
    src/platform/common/dns_model.cpp
)

target_include_directories(platform_network_ops_model_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME platform_network_ops_model_test COMMAND platform_network_ops_model_test)

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

# ===========================================================================
# Phase 1 architecture tests
# ===========================================================================

add_executable(reconnect_policy_test
    tests/reconnect_policy_test.cpp
    src/core/reconnect_policy.cpp
)

target_include_directories(reconnect_policy_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME reconnect_policy_test COMMAND reconnect_policy_test)

add_executable(reconnect_deterministic_test
    tests/reconnect_deterministic_test.cpp
    src/core/reconnect_policy.cpp
)

target_include_directories(reconnect_deterministic_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME reconnect_deterministic_test COMMAND reconnect_deterministic_test)

add_executable(startup_failure_test
    tests/startup_failure_test.cpp
    src/core/native_startup_failure.cpp
)

target_include_directories(startup_failure_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME startup_failure_test COMMAND startup_failure_test)

add_executable(core_error_mapper_test
    tests/core_error_mapper_test.cpp
    src/core/core_error_mapper.cpp
)

target_include_directories(core_error_mapper_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME core_error_mapper_test COMMAND core_error_mapper_test)

add_executable(tunnel_controller_state_machine_test
    tests/tunnel_controller_state_machine_test.cpp
)

target_include_directories(tunnel_controller_state_machine_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME tunnel_controller_state_machine_test COMMAND tunnel_controller_state_machine_test)

add_executable(core_process_test
    tests/core_process_test.cpp
    src/core/tunnel_controller.cpp
    src/core/timing.cpp
    src/core/core_error_mapper.cpp
    src/core/reconnect_policy.cpp
    src/core_api/app_rpc_dispatcher.cpp
    src/core_api/vpn_actions.cpp
    src/core_api/config_actions.cpp
    src/core_api/service_actions.cpp
    src/core_api/route_actions.cpp
    src/core_api/core_api_setup.cpp
    src/helper_common/helper_messages.cpp
)

target_include_directories(core_process_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

target_link_libraries(core_process_test PRIVATE Threads::Threads)

add_test(NAME core_process_test COMMAND core_process_test)

add_executable(tunnel_controller_integration_test
    tests/tunnel_controller_integration_test.cpp
    tests/support/fake_helper.cpp
    tests/support/fake_platform_network_ops.cpp
    tests/support/fake_core_ui_client.cpp
    src/core/reconnect_policy.cpp
)

target_include_directories(tunnel_controller_integration_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME tunnel_controller_integration_test COMMAND tunnel_controller_integration_test)

add_executable(helper_lease_manager_test
    tests/helper_lease_manager_test.cpp
    src/helper_runtime/session_lease_manager.cpp
)

target_include_directories(helper_lease_manager_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME helper_lease_manager_test COMMAND helper_lease_manager_test)

add_executable(helper_cleanup_registry_test
    tests/helper_cleanup_registry_test.cpp
    src/helper_runtime/cleanup_registry.cpp
)

target_include_directories(helper_cleanup_registry_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME helper_cleanup_registry_test COMMAND helper_cleanup_registry_test)

add_executable(helper_v2_contract_test
    tests/helper_v2_contract_test.cpp
    tests/support/fake_helper.cpp
    src/helper_common/helper_messages.cpp
    src/helper_runtime/session_lease_manager.cpp
    src/helper_runtime/cleanup_registry.cpp
    src/helper_runtime/command_validator.cpp
    src/helper_runtime/helper_lifecycle_policy.cpp
)

target_include_directories(helper_v2_contract_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
    ${CMAKE_SOURCE_DIR}/tests
)

add_test(NAME helper_v2_contract_test COMMAND helper_v2_contract_test)

add_executable(app_api_rpc_dispatcher_test
    tests/app_api_rpc_dispatcher_test.cpp
    src/core_api/app_rpc_dispatcher.cpp
)

target_include_directories(app_api_rpc_dispatcher_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME app_api_rpc_dispatcher_test COMMAND app_api_rpc_dispatcher_test)

# --- Phase 4-6: core_api action tests ---

add_executable(vpn_actions_test
    tests/core_api/vpn_actions_test.cpp
    src/core_api/app_rpc_dispatcher.cpp
    src/core_api/vpn_actions.cpp
    src/core/tunnel_controller.cpp
    src/core/timing.cpp
    src/core/core_error_mapper.cpp
    src/core/reconnect_policy.cpp
    src/helper_common/helper_messages.cpp
    tests/support/fake_helper.cpp
    tests/support/fake_platform_network_ops.cpp
)

target_include_directories(vpn_actions_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
    ${CMAKE_SOURCE_DIR}/tests
)

target_link_libraries(vpn_actions_test PRIVATE Threads::Threads)

add_test(NAME vpn_actions_test COMMAND vpn_actions_test)

add_executable(config_actions_test
    tests/core_api/config_actions_test.cpp
    src/core_api/app_rpc_dispatcher.cpp
    src/core_api/config_actions.cpp
)

target_include_directories(config_actions_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME config_actions_test COMMAND config_actions_test)

add_executable(service_actions_test
    tests/core_api/service_actions_test.cpp
    src/core_api/app_rpc_dispatcher.cpp
    src/core_api/service_actions.cpp
)

target_include_directories(service_actions_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME service_actions_test COMMAND service_actions_test)

add_executable(route_actions_test
    tests/core_api/route_actions_test.cpp
    src/core_api/app_rpc_dispatcher.cpp
    src/core_api/route_actions.cpp
)

target_include_directories(route_actions_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME route_actions_test COMMAND route_actions_test)

add_executable(error_contract_test
    tests/core_api/error_contract_test.cpp
    src/feedback/error_contract.cpp
)

target_include_directories(error_contract_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME error_contract_test COMMAND error_contract_test)

add_executable(native_core_connect_flow_test
    tests/integration/native_core_connect_flow_test.cpp
    tests/support/fake_helper.cpp
    tests/support/fake_platform_network_ops.cpp
)

target_include_directories(native_core_connect_flow_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
    ${CMAKE_SOURCE_DIR}/tests
)

add_test(NAME native_core_connect_flow_test COMMAND native_core_connect_flow_test)

add_executable(helper_timeout_cleanup_test
    tests/integration/helper_timeout_cleanup_test.cpp
    src/helper_runtime/session_lease_manager.cpp
    src/helper_runtime/helper_lifecycle_policy.cpp
)

target_include_directories(helper_timeout_cleanup_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME helper_timeout_cleanup_test COMMAND helper_timeout_cleanup_test)

add_executable(no_secret_in_argv_test
    tests/security/no_secret_in_argv_test.cpp
)

target_include_directories(no_secret_in_argv_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME no_secret_in_argv_test COMMAND no_secret_in_argv_test)

add_executable(no_secret_in_logs_test
    tests/security/no_secret_in_logs_test.cpp
    src/core/core_error_mapper.cpp
)


target_include_directories(no_secret_in_logs_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME no_secret_in_logs_test COMMAND no_secret_in_logs_test)

add_executable(credential_store_model_test
    tests/credential_store_model_test.cpp
    src/platform/common/credential_store.cpp
)

if(WIN32)
    target_sources(credential_store_model_test PRIVATE src/platform/win32/win_credential_store.cpp)
    target_link_libraries(credential_store_model_test PRIVATE advapi32)
elseif(APPLE)
    target_sources(credential_store_model_test PRIVATE src/platform/darwin/darwin_keychain_store.cpp)
    target_link_libraries(credential_store_model_test PRIVATE "-framework Security" "-framework CoreFoundation")
endif()

target_include_directories(credential_store_model_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME credential_store_model_test COMMAND credential_store_model_test)

add_executable(helper_messages_connector_test
    tests/helper_messages_connector_test.cpp
    src/helper_common/helper_messages.cpp
    src/helper_common/helper_connector.cpp
    src/helper_common/pipe_helper_client.cpp
)

target_include_directories(helper_messages_connector_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME helper_messages_connector_test COMMAND helper_messages_connector_test)

# IPC transport test: PipeHelperClient <-> raw platform server (no daemon deps)
add_executable(pipe_helper_client_test
    tests/pipe_helper_client_test.cpp
    src/helper_common/helper_messages.cpp
    src/helper_common/helper_connector.cpp
    src/helper_common/pipe_helper_client.cpp
)

target_include_directories(pipe_helper_client_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
target_link_libraries(pipe_helper_client_test PRIVATE Threads::Threads)

add_test(NAME pipe_helper_client_test COMMAND pipe_helper_client_test)

# ===========================================================================
# Additional test targets (Phase 4-7: protocol, auth, integration)
# ===========================================================================

add_executable(native_authenticator_test
    tests/native_authenticator_test.cpp
    src/vpn_engine/protocol/native_authenticator.cpp
    src/vpn_engine/protocol/auth.cpp
    src/vpn_engine/protocol/http.cpp
    src/vpn_engine/session_state.cpp
)

target_include_directories(native_authenticator_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME native_authenticator_test COMMAND native_authenticator_test)

add_executable(native_auth_session_json_test
    tests/native_auth_session_json_test.cpp
    src/vpn_engine/protocol/native_auth_session_json.cpp
)

target_include_directories(native_auth_session_json_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME native_auth_session_json_test COMMAND native_auth_session_json_test)

add_executable(vpn_supervisor_payload_test
    tests/vpn_supervisor_payload_test.cpp
    src/vpn_engine/protocol/native_auth_session_json.cpp
    src/platform/common/vpn_supervisor_process.cpp
)

target_include_directories(vpn_supervisor_payload_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

add_test(NAME vpn_supervisor_payload_test COMMAND vpn_supervisor_payload_test)

# NOTE: connection_attempt_test, app_api_logs_test, and
# app_api_native_orchestration_test depend on the full app_api build chain
# (WebUI assets, SSE broadcaster, etc.) and cannot be compiled as standalone
# test targets.  They are tested as part of the exv integration build.

add_executable(auth_failure_test
    tests/integration/auth_failure_test.cpp
    src/core/reconnect_policy.cpp
    tests/support/fake_helper.cpp
)

target_include_directories(auth_failure_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
    ${CMAKE_SOURCE_DIR}/tests
)

add_test(NAME auth_failure_test COMMAND auth_failure_test)

add_executable(helper_lost_test
    tests/integration/helper_lost_test.cpp
    src/core/reconnect_policy.cpp
    tests/support/fake_helper.cpp
    tests/support/fake_platform_network_ops.cpp
)

target_include_directories(helper_lost_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
    ${CMAKE_SOURCE_DIR}/tests
)

add_test(NAME helper_lost_test COMMAND helper_lost_test)

add_executable(helper_delegating_network_ops_test
    tests/helper_delegating_network_ops_test.cpp
    tests/support/fake_helper.cpp
    src/helper_common/helper_messages.cpp
    src/platform/common/helper_delegating_network_ops.cpp
)

target_include_directories(helper_delegating_network_ops_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
    ${CMAKE_SOURCE_DIR}/tests
)

add_test(NAME helper_delegating_network_ops_test COMMAND helper_delegating_network_ops_test)

# ===========================================================================
# Test labels for selective test runs
# ===========================================================================
#
# Label taxonomy (CI release gate):
#   release-blocking  -- core logic, must pass for PR merge
#   env-sensitive     -- platform/DLL-dependent, informational in CI
#   integration       -- cross-module, may need helper/network
#   elevated-manual   -- requires admin/root, skipped in CI
#   legacy            -- pre-existing tests with known issues
#
# Functional labels (for local dev filtering):
#   architecture, security, core, helper, platform, ui-contract
#
# NOTE: We use set_property(TEST ... APPEND PROPERTY LABELS ...) so that a
# test can carry multiple labels (e.g. both "release-blocking" and "core").
# Plain set_tests_properties(... LABELS ...) would overwrite prior labels.
# ===========================================================================

# --- Release-blocking: core logic tests (must pass in CI) ---
set(_release_blocking_tests
    vpn_runtime_test
    platform_status_models_test
    platform_network_ops_model_test
    feedback_test
    runtime_status_native_test
    backend_resolver_test
    openconnect_log_test
    native_session_state_test
    native_helper_session_test
    native_engine_contract_test
    native_event_sink_test
    tunnel_script_contract_test
    proxy_tun_detector_test
    app_api_runtime_policy_test
    native_packaging_policy_test
    crypto_roundtrip_test
    native_url_test
    native_cstp_frame_test
    native_tls_stream_contract_test
    native_auth_parser_test
    native_protocol_session_test
    native_production_transport_test
    native_authenticator_test
    native_auth_session_json_test
    vpn_supervisor_payload_test
    reconnect_policy_test
    reconnect_deterministic_test
    startup_failure_test
    core_error_mapper_test
    tunnel_controller_state_machine_test
    helper_lease_manager_test
    helper_cleanup_registry_test
    helper_v2_contract_test
    no_secret_in_argv_test
    no_secret_in_logs_test
    credential_store_model_test
    app_api_rpc_dispatcher_test
    vpn_actions_test
    config_actions_test
    service_actions_test
    route_actions_test
    error_contract_test
    helper_messages_connector_test
    core_process_test
    helper_delegating_network_ops_test
)
set_property(TEST ${_release_blocking_tests} APPEND PROPERTY LABELS "release-blocking")

# Global MinGW DLL fix: ensure all release-blocking tests can find MinGW
# runtime DLLs.  On CI runners the toolchain bin dir is usually already on
# PATH, but local developer machines may not have it.  We inject both the
# MinGW compiler bin directory and the exv-core build directory (which
# contains the MinGW DLLs next to the test binaries) via the ENVIRONMENT
# test property.
if(WIN32 AND NOT MSVC)
    get_filename_component(MINGW_BIN_DIR "${CMAKE_CXX_COMPILER}" DIRECTORY)
    foreach(_test IN LISTS _release_blocking_tests)
        set_tests_properties(${_test} PROPERTIES
            ENVIRONMENT "PATH=${MINGW_BIN_DIR};$<TARGET_FILE_DIR:exv-core>;$ENV{PATH}"
        )
    endforeach()
endif()

# --- Env-sensitive: platform/DLL-dependent tests (informational in CI) ---
# These tests depend on platform-specific DLLs (WinTun, SChannel, etc.) or
# MinGW runtime DLLs that may not be available in all CI environments.
# On Windows+MinGW they may fail with 0xc0000139 (DLL not found).
set_property(TEST
    native_fake_anyconnect_server_test
    APPEND PROPERTY LABELS "env-sensitive"
)

if(WIN32)
    set_property(TEST
        win32_native_wintun_test
        win32_native_ip_config_test
        win32_native_packet_device_test
        win32_native_tls_stream_test
        APPEND PROPERTY LABELS "env-sensitive"
    )
endif()

if(APPLE)
    set_property(TEST
        darwin_native_utun_test
        darwin_native_route_config_test
        darwin_native_packet_device_test
        darwin_native_tls_stream_test
        APPEND PROPERTY LABELS "env-sensitive"
    )
endif()

# --- Integration tests ---
set_property(TEST
    tunnel_controller_integration_test
    native_core_connect_flow_test
    helper_timeout_cleanup_test
    auth_failure_test
    helper_lost_test
    APPEND PROPERTY LABELS "integration"
)

# --- Legacy: pre-existing tests with known issues ---
# app_api_native_orchestration_test has hardcoded test passwords and depends
# on the full native engine build chain.  Isolated until migrated.
if(TARGET app_api_native_orchestration_test)
    set_property(TEST app_api_native_orchestration_test APPEND PROPERTY LABELS "legacy")
endif()

# --- Functional labels (for local dev filtering) ---

# All Phase 1+ architecture tests
set_property(TEST
    reconnect_policy_test
    core_error_mapper_test
    tunnel_controller_state_machine_test
    tunnel_controller_integration_test
    helper_lease_manager_test
    helper_cleanup_registry_test
    helper_v2_contract_test
    app_api_rpc_dispatcher_test
    native_core_connect_flow_test
    helper_timeout_cleanup_test
    no_secret_in_argv_test
    no_secret_in_logs_test
    credential_store_model_test
    helper_messages_connector_test
    platform_network_ops_model_test
    vpn_actions_test
    config_actions_test
    service_actions_test
    route_actions_test
    error_contract_test
    auth_failure_test
    helper_lost_test
    native_authenticator_test
    native_auth_session_json_test
    core_process_test
    helper_delegating_network_ops_test
    APPEND PROPERTY LABELS "architecture"
)

# Security-focused tests
set_property(TEST
    no_secret_in_argv_test
    no_secret_in_logs_test
    credential_store_model_test
    APPEND PROPERTY LABELS "security"
)

# Core module tests
set_property(TEST
    reconnect_policy_test
    core_error_mapper_test
    tunnel_controller_state_machine_test
    tunnel_controller_integration_test
    core_process_test
    APPEND PROPERTY LABELS "core"
)

# Helper subsystem tests
set_property(TEST
    helper_lease_manager_test
    helper_cleanup_registry_test
    helper_v2_contract_test
    helper_messages_connector_test
    helper_timeout_cleanup_test
    APPEND PROPERTY LABELS "helper"
)

# Platform abstraction tests
set_property(TEST
    platform_network_ops_model_test
    credential_store_model_test
    helper_delegating_network_ops_test
    APPEND PROPERTY LABELS "platform"
)

# UI / RPC contract tests
set_property(TEST
    app_api_rpc_dispatcher_test
    vpn_actions_test
    config_actions_test
    service_actions_test
    route_actions_test
    error_contract_test
    core_process_test
    APPEND PROPERTY LABELS "ui-contract"
)

# Install target
if(WIN32)
  install(TARGETS exv exv-helper DESTINATION "C:/Program Files/ECNU-VPN")
else()
  install(TARGETS exv exv-helper DESTINATION /usr/local/bin)
endif()
