cmake_minimum_required(VERSION 3.28)
project(exv VERSION 3.3.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_SCAN_FOR_MODULES OFF)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)


option(EXV_BUILD_UI_SHELL
    "Require real platform WebView shell SDK dependencies instead of stub host builds"
    OFF
)

# Release builds optimise for size and strip debug symbols. The MinGW
# toolchain we ship on Windows links against the gcc/g++ runtime DLLs, so
# stripping the binary itself gives the bulk of the savings (~3-4MB).
if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release CACHE STRING "" FORCE)
endif()

# Force UTF-8 source encoding for MSVC
if(MSVC)
    add_compile_options(/utf-8)
    add_compile_definitions(NOMINMAX)  # Prevent min/max macro conflicts
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
    src/vpn_engine/protocol/aggregate_auth.cpp
    src/vpn_engine/protocol/auth.cpp
    src/vpn_engine/protocol/cstp.cpp
    src/vpn_engine/protocol/dtls_transport.cpp
    src/vpn_engine/protocol/http.cpp
    src/vpn_engine/protocol/production_transport.cpp
    src/vpn_engine/protocol/session.cpp
    src/vpn_engine/protocol/tls_stream.cpp
    src/vpn_engine/protocol/url.cpp
)

set(EXV_OBSERVABILITY_SOURCES
    src/observability/log_queue.cpp
    src/observability/log_service.cpp
    src/observability/log_facade.cpp
)

set(EXV_PLATFORM_LOGGING_SOURCES
    src/platform/common/logging/file_log_sink.cpp
    src/platform/common/logging/stdout_log_sink.cpp
    src/platform/common/logging/log_runtime.cpp
)

set(EXV_CORE_SOURCES
    src/core/config/config_manager.cpp
    src/core/config/config_initialization.cpp
    src/core/config/config_api.cpp
    src/core/config/config_persistence.cpp
    src/core/config/config_show.cpp
    src/core/config/config_set_value.cpp
    src/core/config/config_routes.cpp
    src/core/config/config_key.cpp
    src/core/config/config_wizard_common.cpp
    src/core/core_process.cpp
    src/platform/common/lifecycle/core_identity.cpp
    src/platform/common/lifecycle/core_paths.cpp
    src/platform/common/lifecycle/core_lock.cpp
    src/platform/common/lifecycle/core_registry.cpp
    src/platform/common/core_resolver.cpp
    src/core/pipe_ipc.cpp
    src/core/tunnel_controller/engine_event_bridge.cpp
    src/core/tunnel_controller/core_session_runner.cpp
    src/core/tunnel_controller/tunnel_controller.cpp
    src/core/tunnel_controller/tunnel_controller_state.cpp
    src/core/tunnel_controller/tunnel_controller_heartbeat.cpp
    src/core/tunnel_controller/tunnel_controller_connect.cpp
    src/core/tunnel_controller/tunnel_controller_disconnect.cpp
    src/core/tunnel_controller/tunnel_controller_reconnect.cpp
    src/core/tunnel_controller/tunnel_controller_events.cpp
    src/core/tunnel_controller/tunnel_controller_facade.cpp
    src/core/tunnel_controller/tunnel_controller_active.cpp
    src/core/tunnel_controller/connect_intent.cpp
    src/core/tunnel_controller/vpn_connect_job.cpp
    src/core/tunnel_controller/connect_pipeline.cpp
    src/core/tunnel_controller/timing.cpp
    src/core/tunnel_controller/core_error_mapper.cpp
    src/core/tunnel_controller/native_engine_config_mapper.cpp
    src/core/tunnel_controller/reconnect_policy.cpp
    src/core/tunnel_controller/timer_scheduler.cpp
    src/core/rpc/app_rpc_dispatcher.cpp
    src/core/rpc/rpc_action_metadata.cpp
    src/core/rpc/lane_scheduler.cpp
    src/core/rpc/core_actions.cpp
    src/core/rpc/desktop_rpc_adapter.cpp
    src/core/rpc/vpn_actions.cpp
    src/core/rpc/config_actions.cpp
    src/core/rpc/service_actions.cpp
    src/core/rpc/route_actions.cpp
    src/core/rpc/log_actions.cpp
    src/core/rpc/maintenance_actions.cpp
    src/core/rpc/core_api_setup.cpp
    src/core/use_cases/config_use_cases.cpp
    src/core/use_cases/system_status_use_cases.cpp
    src/core/crypto/crypto.cpp
    src/helper/common/helper_connector.cpp
    src/helper/common/pipe_helper_client.cpp
    src/platform/common/backend_resolver.cpp
    src/platform/common/credential_store.cpp
    src/platform/common/dns_model.cpp
    src/platform/common/file_system.cpp
    src/platform/common/interface_stats.cpp
    src/platform/common/oneshot_bootstrap.cpp
    src/platform/common/packet_device_factory.cpp
    src/platform/common/process_utils.cpp
    src/platform/common/platform_network_ops.cpp
    src/platform/common/runtime_discovery.cpp
    src/platform/common/runtime_paths.cpp
    src/platform/common/helper_delegating_network_ops.cpp
    src/platform/common/proxy_tun_detector.cpp
    src/platform/common/route_model.cpp
    src/platform/common/status_models.cpp
    src/platform/common/runtime_status.cpp
    src/vpn_engine/native_engine.cpp
    src/vpn_engine/native_handshake_job.cpp
    ${EXV_PROTOCOL_SOURCES}
    src/vpn_engine/session_state.cpp
    src/core/app_api/app_api.cpp
    src/core/app_api/auth_interaction_coordinator.cpp
    src/core/app_api/desktop_action_registry.cpp
    src/core/app_api/desktop_config_actions.cpp
    src/core/app_api/desktop_json.cpp
    src/core/app_api/desktop_log_actions.cpp
    src/core/app_api/desktop_route_actions.cpp
    src/core/app_api/desktop_runtime_context.cpp
    src/core/app_api/desktop_status_presenter.cpp
    src/core/app_api/desktop_system_actions.cpp
    src/core/app_api/desktop_tunnel_host.cpp
    src/core/app_api/desktop_vpn_actions.cpp
    src/core/vpn/vpn.cpp
    src/core/vpn/vpn_runtime.cpp
    src/core/network/virtual_network_status.cpp
    src/runtime/runtime_context.cpp
    src/feedback/feedback.cpp
    src/core/connection/connection_attempt.cpp
    src/common/diagnostics/logger.cpp
    src/common/diagnostics/log_event_bus.cpp
    src/common/diagnostics/log_renderer.cpp
    ${EXV_OBSERVABILITY_SOURCES}
    ${EXV_PLATFORM_LOGGING_SOURCES}
    src/cli/console.cpp
)

set(EXV_HELPER_RUNTIME_SOURCES
    src/helper/helper.cpp
    src/helper/helper_daemon_context.cpp
    src/helper/helper_network_ops.cpp
    src/helper/helper_service_ops.cpp
    src/helper/common/helper_messages.cpp
    src/helper/runtime/helper_request_dispatcher.cpp
    src/helper/runtime/session_lease_manager.cpp
    src/helper/runtime/cleanup_registry.cpp
    src/helper/runtime/command_validator.cpp
    src/helper/runtime/helper_lifecycle_policy.cpp
    src/helper/runtime/helper_session_store.cpp
    src/helper/runtime/privileged_task_queue.cpp
    src/helper/helper_handler.cpp
)

set(EXV_CLI_TRANSPORT_SOURCES
    src/cli/pipe_client.cpp
)

set(EXV_CLI_FRONTEND_SOURCES
    src/cli/cli_main.cpp
    src/cli/cli_entrypoint.cpp
    src/cli/cli_commands.cpp
    src/cli/core_request_formatter.cpp
    src/cli/console.cpp
    ${EXV_CLI_TRANSPORT_SOURCES}
    src/platform/common/core_resolver.cpp
    src/platform/common/core_resolver_platform_deps.cpp
    src/platform/common/file_system.cpp
    src/platform/common/lifecycle/core_identity.cpp
    src/platform/common/lifecycle/core_lock.cpp
    src/platform/common/lifecycle/core_paths.cpp
    src/platform/common/lifecycle/core_registry.cpp
    src/platform/common/process_utils.cpp
    src/platform/common/runtime_paths.cpp
    src/runtime/runtime_context.cpp
    src/observability/log_facade.cpp
    src/observability/log_queue.cpp
    src/observability/log_service.cpp
    src/utils/strings.cpp
)

set(EXV_PLATFORM_SOURCES)
set(EXV_PLATFORM_LIBS)
set(EXV_PLATFORM_DEFINITIONS)

# Source files
set(SOURCES
    src/app/main.cpp
    src/cli/cli_entrypoint.cpp
    src/cli/cli_commands.cpp
    src/cli/core_request_formatter.cpp
    src/cli/pipe_client.cpp
    src/platform/common/core_resolver_platform_deps.cpp
)
find_package(Threads REQUIRED)

# Platform-specific dependencies
if(APPLE)
    # CommonCrypto links implicitly on macOS
    list(APPEND EXV_PLATFORM_DEFINITIONS EXV_PLATFORM_DARWIN=1)
    list(APPEND EXV_PLATFORM_LIBS "-framework Security" "-framework CoreFoundation")
    list(APPEND EXV_PLATFORM_SOURCES
        src/platform/darwin/helper_daemon.cpp
        src/platform/darwin/app_api_runtime_policy.cpp
        src/platform/darwin/crypto_backend.cpp
        src/platform/darwin/config_defaults.cpp
        src/platform/darwin/helper_client.cpp
        src/platform/darwin/helper_lifecycle.cpp
        src/platform/darwin/helper_platform.cpp
        src/platform/darwin/helper_service_manager.cpp
        src/platform/darwin/native_tls_stream.cpp
        src/platform/darwin/native_packet_device.cpp
        src/platform/darwin/default_engine_deps.cpp
        src/platform/darwin/native_route_config.cpp
        src/platform/darwin/native_utun.cpp
        src/platform/darwin/platform_network_ops_darwin.cpp
        src/platform/darwin/oneshot_bootstrap.cpp
        src/platform/darwin/process_control.cpp
        src/platform/darwin/virtual_network_probe.cpp
        src/platform/darwin/path_utils.cpp
        src/platform/darwin/service_status.cpp
        src/platform/darwin/darwin_keychain_store.cpp
        src/platform/common/driver_status_stub.cpp
    )
    list(APPEND EXV_CLI_FRONTEND_SOURCES
        src/platform/darwin/crypto_backend.cpp
        src/platform/darwin/path_utils.cpp
    )
elseif(UNIX AND NOT APPLE)
    find_package(OpenSSL 1.1.1 REQUIRED)
    list(APPEND EXV_PLATFORM_DEFINITIONS EXV_PLATFORM_LINUX=1)
    list(APPEND EXV_PLATFORM_LIBS OpenSSL::SSL OpenSSL::Crypto)
    list(APPEND EXV_PLATFORM_SOURCES
        src/platform/linux/helper_daemon.cpp
        src/platform/linux/app_api_runtime_policy.cpp
        src/platform/linux/crypto_backend.cpp
        src/platform/linux/config_defaults.cpp
        src/platform/linux/helper_client.cpp
        src/platform/linux/helper_lifecycle.cpp
        src/platform/linux/helper_platform.cpp
        src/platform/linux/helper_service_manager.cpp
        src/platform/linux/oneshot_bootstrap.cpp
        src/platform/linux/platform_network_ops_linux.cpp
        src/platform/linux/process_control.cpp
        src/platform/linux/virtual_network_probe.cpp
        src/platform/linux/path_utils.cpp
        src/platform/linux/default_engine_deps.cpp
        src/platform/common/service_status_linux.cpp
        src/platform/common/driver_status_stub.cpp
    )
    list(APPEND EXV_CLI_FRONTEND_SOURCES
        src/platform/linux/crypto_backend.cpp
        src/platform/linux/path_utils.cpp
    )
elseif(WIN32)
    # AES-256-CBC and CSPRNG come from Windows CNG (bcrypt) so we no longer
    # need OpenSSL on this platform. The ~7MB libssl/libcrypto DLLs can be
    # dropped from the desktop bundle entirely.
    list(APPEND EXV_PLATFORM_DEFINITIONS EXV_PLATFORM_WINDOWS=1)
    list(APPEND EXV_PLATFORM_LIBS bcrypt advapi32 iphlpapi ws2_32 secur32 crypt32)
    list(APPEND EXV_PLATFORM_SOURCES
        src/platform/win32/helper_daemon.cpp
        src/platform/win32/app_api_runtime_policy.cpp
        src/platform/win32/crypto_backend.cpp
        src/platform/win32/config_defaults.cpp
        src/platform/win32/helper_client.cpp
        src/platform/win32/helper_lifecycle.cpp
        src/platform/win32/helper_platform.cpp
        src/platform/win32/helper_service_manager.cpp
        src/platform/win32/native_tls_stream.cpp
        src/platform/win32/native_packet_device.cpp
        src/platform/win32/default_engine_deps.cpp
        src/platform/win32/native_wintun.cpp
        src/platform/win32/native_ip_config.cpp
        src/platform/win32/platform_network_ops_win32.cpp
        src/platform/win32/oneshot_bootstrap.cpp
        src/platform/win32/process_control.cpp
        src/platform/win32/virtual_network_probe.cpp
        src/platform/win32/path_utils.cpp
        src/platform/win32/service_status.cpp
        src/platform/win32/driver_status.cpp
        src/platform/win32/win_route_ops.cpp
        src/platform/win32/win_credential_store.cpp
        src/platform/win32/windows_strings.cpp
    )
    list(APPEND EXV_CLI_FRONTEND_SOURCES
        src/platform/win32/crypto_backend.cpp
        src/platform/win32/path_utils.cpp
    )
else()
    message(FATAL_ERROR "Unsupported platform. Supported: macOS, Linux, Windows.")
endif()

# This is the first native split toward the documented architecture.  The
# platform adapter files still live in this target because the current core
# sources call platform functions directly; C1/C2 will split those adapters
# behind cleaner interfaces.
add_library(exv-core STATIC
    ${EXV_CORE_SOURCES}
    ${EXV_PLATFORM_SOURCES}
)
target_include_directories(exv-core PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
target_link_libraries(exv-core PUBLIC Threads::Threads ${EXV_PLATFORM_LIBS})
target_compile_definitions(exv-core PUBLIC
    EXV_VERSION="${PROJECT_VERSION}"
    ${EXV_PLATFORM_DEFINITIONS}
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
    EXV_VERSION="${PROJECT_VERSION}"
    ${EXV_PLATFORM_DEFINITIONS}
)

add_library(exv-base-types-module STATIC)
target_sources(exv-base-types-module
    PUBLIC
        FILE_SET CXX_MODULES
        FILES
            src/base/modules/types.cppm
)
target_compile_features(exv-base-types-module PUBLIC cxx_std_20)
target_include_directories(exv-base-types-module PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
set_target_properties(exv-base-types-module PROPERTIES
    CXX_SCAN_FOR_MODULES ON
)

add_library(exv-base-errors-module STATIC)
target_sources(exv-base-errors-module
    PUBLIC
        FILE_SET CXX_MODULES
        FILES
            src/base/modules/errors.cppm
)
target_compile_features(exv-base-errors-module PUBLIC cxx_std_20)
target_include_directories(exv-base-errors-module PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
set_target_properties(exv-base-errors-module PROPERTIES
    CXX_SCAN_FOR_MODULES ON
)

add_library(exv-observability-log-module STATIC)
target_sources(exv-observability-log-module
    PUBLIC
        FILE_SET CXX_MODULES
        FILES
            src/observability/modules/log.cppm
)
target_compile_features(exv-observability-log-module PUBLIC cxx_std_20)
target_include_directories(exv-observability-log-module PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
set_target_properties(exv-observability-log-module PROPERTIES
    CXX_SCAN_FOR_MODULES ON
)

add_library(exv-helper-protocol-module STATIC)
target_sources(exv-helper-protocol-module
    PUBLIC
        FILE_SET CXX_MODULES
        FILES
            src/helper/modules/helper_protocol.cppm
)
target_compile_features(exv-helper-protocol-module PUBLIC cxx_std_20)
set_target_properties(exv-helper-protocol-module PROPERTIES
    CXX_SCAN_FOR_MODULES ON
)

add_library(exv-config-contract-module STATIC)
target_sources(exv-config-contract-module
    PUBLIC
        FILE_SET CXX_MODULES
        FILES
            src/core/config/modules/config_contract.cppm
)
target_compile_features(exv-config-contract-module PUBLIC cxx_std_20)
set_target_properties(exv-config-contract-module PROPERTIES
    CXX_SCAN_FOR_MODULES ON
)

add_library(exv-core-tunnel-contract-module STATIC)
target_sources(exv-core-tunnel-contract-module
    PUBLIC
        FILE_SET CXX_MODULES
        FILES
            src/core/tunnel_controller/modules/tunnel_contract.cppm
)
target_compile_features(exv-core-tunnel-contract-module PUBLIC cxx_std_20)
set_target_properties(exv-core-tunnel-contract-module PROPERTIES
    CXX_SCAN_FOR_MODULES ON
)

add_library(exv-core-tunnel-reconnect-module STATIC)
target_sources(exv-core-tunnel-reconnect-module
    PUBLIC
        FILE_SET CXX_MODULES
        FILES
            src/core/tunnel_controller/modules/tunnel_reconnect.cppm
)
target_compile_features(exv-core-tunnel-reconnect-module PUBLIC cxx_std_20)
target_include_directories(exv-core-tunnel-reconnect-module PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
set_target_properties(exv-core-tunnel-reconnect-module PROPERTIES
    CXX_SCAN_FOR_MODULES ON
)

add_library(exv-core-tunnel-errors-module STATIC)
target_sources(exv-core-tunnel-errors-module
    PUBLIC
        FILE_SET CXX_MODULES
        FILES
            src/core/tunnel_controller/modules/tunnel_errors.cppm
)
target_compile_features(exv-core-tunnel-errors-module PUBLIC cxx_std_20)
target_include_directories(exv-core-tunnel-errors-module PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
set_target_properties(exv-core-tunnel-errors-module PROPERTIES
    CXX_SCAN_FOR_MODULES ON
)

add_library(exv-core-tunnel-events-module STATIC)
target_sources(exv-core-tunnel-events-module
    PUBLIC
        FILE_SET CXX_MODULES
        FILES
            src/core/tunnel_controller/modules/tunnel_events.cppm
)
target_compile_features(exv-core-tunnel-events-module PUBLIC cxx_std_20)
target_include_directories(exv-core-tunnel-events-module PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
set_target_properties(exv-core-tunnel-events-module PROPERTIES
    CXX_SCAN_FOR_MODULES ON
)

add_library(exv-core-tunnel-timing-module STATIC)
target_sources(exv-core-tunnel-timing-module
    PUBLIC
        FILE_SET CXX_MODULES
        FILES
            src/core/tunnel_controller/modules/tunnel_timing.cppm
)
target_compile_features(exv-core-tunnel-timing-module PUBLIC cxx_std_20)
target_include_directories(exv-core-tunnel-timing-module PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
set_target_properties(exv-core-tunnel-timing-module PROPERTIES
    CXX_SCAN_FOR_MODULES ON
)

add_library(exv-core-tunnel-module STATIC)
target_sources(exv-core-tunnel-module
    PUBLIC
        FILE_SET CXX_MODULES
        FILES
            src/core/tunnel_controller/modules/tunnel.cppm
)
target_compile_features(exv-core-tunnel-module PUBLIC cxx_std_20)
target_include_directories(exv-core-tunnel-module PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
set_target_properties(exv-core-tunnel-module PROPERTIES
    CXX_SCAN_FOR_MODULES ON
)

add_library(exv-utils-strings STATIC
    src/utils/strings.cpp
)
target_include_directories(exv-utils-strings PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
target_compile_features(exv-utils-strings PUBLIC cxx_std_20)

add_library(exv-utils-strings-module STATIC)
target_sources(exv-utils-strings-module
    PUBLIC
        FILE_SET CXX_MODULES
        FILES
            src/utils/modules/strings.cppm
)
target_compile_features(exv-utils-strings-module PUBLIC cxx_std_20)
target_include_directories(exv-utils-strings-module PUBLIC
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
target_link_libraries(exv-utils-strings-module PUBLIC
    exv-utils-strings
)
set_target_properties(exv-utils-strings-module PROPERTIES
    CXX_SCAN_FOR_MODULES ON
)

target_link_libraries(exv-core PUBLIC
    exv-utils-strings
)

add_executable(exv ${SOURCES})
# exv-core and exv-helper-runtime form a circular static-library
# dependency: core sources (pipe_helper_client, service_status,
# oneshot_bootstrap) reference helper messages and helper::is_available,
# while helper-runtime depends on core.  GNU ld resolves archives
# left-to-right by default, so the group wrapper tells it to make
# multiple passes.  MSVC and Apple ld64 handle this natively.
if(MINGW)
    target_link_libraries(exv PRIVATE
        "-Wl,--start-group"
        exv-helper-runtime
        exv-core
        "-Wl,--end-group"
    )
else()
    target_link_libraries(exv PRIVATE exv-helper-runtime exv-core)
endif()

add_executable(exv-cli ${EXV_CLI_FRONTEND_SOURCES})
target_link_libraries(exv-cli PRIVATE Threads::Threads ${EXV_PLATFORM_LIBS})
target_compile_definitions(exv-cli PRIVATE
    EXV_VERSION="${PROJECT_VERSION}"
    ${EXV_PLATFORM_DEFINITIONS}
)

add_executable(exv-helper
    src/helper/helper_main.cpp
)
target_link_libraries(exv-helper PRIVATE exv-helper-runtime)
target_compile_definitions(exv-helper PRIVATE
    EXV_VERSION="${PROJECT_VERSION}"
)

# Pass version to source
target_compile_definitions(exv PRIVATE
    EXV_VERSION="${PROJECT_VERSION}"
)


# Native WebView UI shell

if(UNIX AND NOT APPLE AND EXV_BUILD_UI_SHELL)
    find_package(PkgConfig REQUIRED)
    pkg_check_modules(WEBKITGTK REQUIRED webkit2gtk-4.1 gtk+-3.0)
endif()

set(EXV_UI_COMMON_SOURCES
    src/app/ui_shell/ui_shell_main.cpp
    src/app/ui_shell/ui_window.cpp
    src/app/ui_shell/ui_shell_options.cpp
    src/app/ui_shell/renderer_assets.cpp
    src/app/ui_shell/close_preference.cpp
    src/app/ui_shell/host_bridge.cpp
    src/app/ui_shell/async_host_bridge.cpp
    src/app/ui_shell/core_rpc_client.cpp
    src/app/ui_shell/core_process_manager.cpp
    src/app/ui_shell/ui_shell_runtime.cpp
    ${EXV_CLI_TRANSPORT_SOURCES}
    src/platform/common/core_resolver_platform_deps.cpp
)

set(EXV_UI_PLATFORM_SOURCES)
if(WIN32)
    if(EXV_BUILD_UI_SHELL)
        if(NOT DEFINED WEBVIEW2_SDK_DIR OR WEBVIEW2_SDK_DIR STREQUAL "")
            message(FATAL_ERROR "WEBVIEW2_SDK_DIR is required to build the Windows WebView2 shell")
        endif()

        set(WEBVIEW2_INCLUDE_DIR "${WEBVIEW2_SDK_DIR}/build/native/include")
        if(NOT EXISTS "${WEBVIEW2_INCLUDE_DIR}/WebView2.h")
            message(FATAL_ERROR "WEBVIEW2_SDK_DIR does not contain build/native/include/WebView2.h")
        endif()

        set(WEBVIEW2_LOADER_CANDIDATES
            "${WEBVIEW2_SDK_DIR}/build/native/x64/WebView2Loader.dll.lib"
            "${WEBVIEW2_SDK_DIR}/build/native/x64/WebView2LoaderStatic.lib"
            "${WEBVIEW2_SDK_DIR}/build/native/x86/WebView2Loader.dll.lib"
            "${WEBVIEW2_SDK_DIR}/build/native/x86/WebView2LoaderStatic.lib"
            "${WEBVIEW2_SDK_DIR}/build/native/arm64/WebView2Loader.dll.lib"
            "${WEBVIEW2_SDK_DIR}/build/native/arm64/WebView2LoaderStatic.lib"
        )
        set(WEBVIEW2_LOADER_LIBRARY "")
        foreach(_candidate IN LISTS WEBVIEW2_LOADER_CANDIDATES)
            if(EXISTS "${_candidate}")
                set(WEBVIEW2_LOADER_LIBRARY "${_candidate}")
                break()
            endif()
        endforeach()
        if(WEBVIEW2_LOADER_LIBRARY STREQUAL "")
            message(FATAL_ERROR "WEBVIEW2_SDK_DIR does not contain a WebView2Loader library")
        endif()

        set(WEBVIEW2_LOADER_DLL_CANDIDATES
            "${WEBVIEW2_SDK_DIR}/runtimes/win-x64/native/WebView2Loader.dll"
            "${WEBVIEW2_SDK_DIR}/runtimes/win-x86/native/WebView2Loader.dll"
            "${WEBVIEW2_SDK_DIR}/runtimes/win-arm64/native/WebView2Loader.dll"
            "${WEBVIEW2_SDK_DIR}/build/native/x64/WebView2Loader.dll"
            "${WEBVIEW2_SDK_DIR}/build/native/x86/WebView2Loader.dll"
            "${WEBVIEW2_SDK_DIR}/build/native/arm64/WebView2Loader.dll"
        )
        set(WEBVIEW2_LOADER_DLL "")
        foreach(_candidate IN LISTS WEBVIEW2_LOADER_DLL_CANDIDATES)
            if(EXISTS "${_candidate}")
                set(WEBVIEW2_LOADER_DLL "${_candidate}")
                break()
            endif()
        endforeach()
        if(WEBVIEW2_LOADER_DLL STREQUAL "")
            message(FATAL_ERROR "WEBVIEW2_SDK_DIR does not contain WebView2Loader.dll")
        endif()
    endif()
endif()

if(WIN32)
    set_source_files_properties(
        src/platform/win32/ui_shell/exv_ui_win32.rc
        PROPERTIES
        OBJECT_DEPENDS "${CMAKE_SOURCE_DIR}/src/platform/win32/ui_shell/exv_ui_win32.manifest"
    )

    list(APPEND EXV_UI_PLATFORM_SOURCES
        src/platform/win32/ui_shell/exv_ui_win32.rc
        src/platform/win32/ui_shell/webview2_host_win32.cpp
        src/platform/win32/ui_shell/webview2_runtime_win32.cpp
    )
elseif(APPLE)
    list(APPEND EXV_UI_PLATFORM_SOURCES
        src/platform/darwin/ui_shell/wk_webview_host_darwin.mm
    )
elseif(UNIX)
    list(APPEND EXV_UI_PLATFORM_SOURCES
        src/platform/linux/ui_shell/webkitgtk_host_linux.cpp
    )
endif()

add_executable(exv-ui
    ${EXV_UI_COMMON_SOURCES}
    ${EXV_UI_PLATFORM_SOURCES}
)

target_include_directories(exv-ui PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

if(WIN32)
    target_link_libraries(exv-ui PRIVATE advapi32 urlmon shell32 exv-core)
    target_include_directories(exv-ui PRIVATE
        ${CMAKE_SOURCE_DIR}/src/platform/win32/ui_shell
        ${CMAKE_SOURCE_DIR}/assets/icons
    )
    if(EXV_BUILD_UI_SHELL)
        target_compile_definitions(exv-ui PRIVATE EXV_BUILD_UI_SHELL=1)
        target_include_directories(exv-ui PRIVATE "${WEBVIEW2_INCLUDE_DIR}")
        target_link_libraries(exv-ui PRIVATE "${WEBVIEW2_LOADER_LIBRARY}")
        add_custom_command(TARGET exv-ui POST_BUILD
            COMMAND ${CMAKE_COMMAND} -E copy_if_different
                    "${WEBVIEW2_LOADER_DLL}"
                    "$<TARGET_FILE_DIR:exv-ui>/WebView2Loader.dll"
        )
    endif()
elseif(APPLE AND EXV_BUILD_UI_SHELL)
    target_compile_definitions(exv-ui PRIVATE EXV_BUILD_UI_SHELL=1)
    target_link_libraries(exv-ui PRIVATE exv-core)
    target_link_libraries(exv-ui PRIVATE "-framework Cocoa" "-framework WebKit")
elseif(UNIX AND NOT APPLE AND EXV_BUILD_UI_SHELL)
    target_compile_definitions(exv-ui PRIVATE EXV_BUILD_UI_SHELL=1)
    target_include_directories(exv-ui PRIVATE ${WEBKITGTK_INCLUDE_DIRS})
    target_link_directories(exv-ui PRIVATE ${WEBKITGTK_LIBRARY_DIRS})
    target_compile_options(exv-ui PRIVATE ${WEBKITGTK_CFLAGS_OTHER})
    target_link_libraries(exv-ui PRIVATE exv-core)
    target_link_libraries(exv-ui PRIVATE ${WEBKITGTK_LIBRARIES})
endif()


