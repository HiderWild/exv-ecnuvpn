# windows_setup_rust — isolated fork of the EXV custom Windows setup engine for
# the Rust product line (exv-core / exv-engine / exv-ui + wintun.dll).
#
# This is a SELF-CONTAINED CMake project: it does NOT include the EXV C++ product
# CMakeLists. It builds exactly two tools, copied from the deprecated C++ chain and
# adapted for the Rust trio:
#   - exv-setup (WIN32)          — the installer / uninstaller stub
#   - pack_setup_payload         — EXVP01 payload packer + PE resource injector
#
# Build:
#   cmake -S <this dir> -B <build dir> -G Ninja -DCMAKE_BUILD_TYPE=Release \
#         -DEXV_PRODUCT_VERSION=4.0.0
#   cmake --build <build dir> -j 4 --target exv-setup pack_setup_payload

cmake_minimum_required(VERSION 3.20)
project(exv-setup-rust LANGUAGES CXX RC)

include(CTest)

file(READ "${CMAKE_CURRENT_SOURCE_DIR}/../../../../release.json" EXV_RELEASE_JSON)
string(JSON EXV_SOURCE_VERSION GET "${EXV_RELEASE_JSON}" version)
set(EXV_PRODUCT_VERSION "${EXV_SOURCE_VERSION}" CACHE STRING "EXV product version baked into the installer registry entry")
if(NOT EXV_PRODUCT_VERSION STREQUAL EXV_SOURCE_VERSION)
  message(FATAL_ERROR "EXV_PRODUCT_VERSION differs from release.json; configure with the source product version")
endif()

set(EXV_RUST_SETUP_PAYLOAD_SOURCES
  payload/crc32.cpp
  payload/win_compress.cpp
  payload/archive_reader.cpp
  payload/archive_writer.cpp
)

set(EXV_RUST_SETUP_ENGINE_SOURCES
  cli.cpp
  app_paths.cpp
  util/hidden_process.cpp
  process_control.cpp
  service_control.cpp
  elevate.cpp
  registry_install.cpp
  shortcuts.cpp
  progress_model.cpp
  uninstaller_copy.cpp
  install_engine.cpp
  pfro_sweep.cpp
  uninstall_engine.cpp
  payload/embedded_payload.cpp
  ui/setup_window.cpp
  webview2_runtime.cpp
  ui/icon_bitmaps.cpp
  ${EXV_RUST_SETUP_PAYLOAD_SOURCES}
)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Version define: EXV_PRODUCT_VERSION_W=L"4.0.0"
set(EXV_PRODUCT_VERSION_DEF "EXV_PRODUCT_VERSION=L\"${EXV_PRODUCT_VERSION}\"")

# Self-contained assets so the fork builds independently of the worktree asset dir.
set(EXV_ICON_ICO "${CMAKE_CURRENT_SOURCE_DIR}/assets/icon.ico")
set(EXV_ICON_256 "${CMAKE_CURRENT_SOURCE_DIR}/assets/256x256.png")
configure_file(exv-setup.rc.in ${CMAKE_CURRENT_BINARY_DIR}/exv-setup.rc @ONLY)

# Sources include "windows_setup_rust/xxx.hpp", so the include root is the parent
# dir that contains the windows_setup_rust/ folder (src/platform/win32).
set(EXV_RUST_SETUP_INCLUDE_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/..")

add_executable(exv-setup WIN32
  main.cpp
  ${CMAKE_CURRENT_BINARY_DIR}/exv-setup.rc
  ${EXV_RUST_SETUP_ENGINE_SOURCES}
)
target_include_directories(exv-setup PRIVATE
  ${EXV_RUST_SETUP_INCLUDE_ROOT}   # resolves windows_setup_rust/xxx.hpp includes
  ${CMAKE_CURRENT_SOURCE_DIR}      # resolves resource.hpp from the configured .rc
)
target_compile_definitions(exv-setup PRIVATE ${EXV_PRODUCT_VERSION_DEF})
target_link_libraries(exv-setup PRIVATE
  shell32 advapi32 ole32 uuid cabinet d2d1 dwrite windowscodecs dwmapi msimg32
)
set_target_properties(exv-setup PROPERTIES
  WIN32_EXECUTABLE TRUE
  OUTPUT_NAME "exv-setup"
)
if(MINGW)
  # 发布安装器沿用旧产品的体积优化；静态运行库仍支持干净宿主。
  target_compile_options(exv-setup PRIVATE
    "$<$<CONFIG:Release>:-Os>"
    "$<$<CONFIG:Release>:-ffunction-sections>"
    "$<$<CONFIG:Release>:-fdata-sections>"
  )
  target_link_options(exv-setup PRIVATE
    "$<$<CONFIG:Release>:-Wl,--gc-sections>"
    "$<$<CONFIG:Release>:-Wl,-s>"
  )
  # Single-file installer runs on clean machines without MinGW runtime DLLs.
  target_link_options(exv-setup PRIVATE
    -municode
    -static
    -static-libgcc
    -static-libstdc++
  )
endif()

if(BUILD_TESTING)
  add_executable(service_stop_test tests/service_stop_test.cpp)
  target_include_directories(service_stop_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  target_link_libraries(service_stop_test PRIVATE advapi32)
  if(MINGW)
    target_link_options(service_stop_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME installer_service_stop_budget COMMAND service_stop_test)
  set_tests_properties(installer_service_stop_budget PROPERTIES TIMEOUT 10)
  add_executable(setup_cli_options_test p1_setup_regression_test.cpp cli.cpp app_paths.cpp registry_install.cpp)
  target_include_directories(setup_cli_options_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  target_link_libraries(setup_cli_options_test PRIVATE shell32 ole32 advapi32)
  if(MINGW)
    target_link_options(setup_cli_options_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME setup_cli_elevation_context COMMAND setup_cli_options_test)
  set_tests_properties(setup_cli_elevation_context PROPERTIES TIMEOUT 10)
  add_executable(webview2_runtime_test tests/webview2_runtime_test.cpp webview2_runtime.cpp)
  target_include_directories(webview2_runtime_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  target_link_libraries(webview2_runtime_test PRIVATE advapi32)
  if(MINGW)
    target_link_options(webview2_runtime_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME webview2_runtime_detection COMMAND webview2_runtime_test)
  set_tests_properties(webview2_runtime_detection PROPERTIES TIMEOUT 10)
  add_executable(install_service_state_test tests/install_service_state_test.cpp progress_model.cpp)
  target_include_directories(install_service_state_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  target_link_libraries(install_service_state_test PRIVATE shell32)
  if(MINGW)
    target_link_options(install_service_state_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME install_service_state_preservation COMMAND install_service_state_test)
  set_tests_properties(install_service_state_preservation PROPERTIES TIMEOUT 10)
  add_executable(uninstall_result_test tests/uninstall_result_test.cpp progress_model.cpp)
  target_include_directories(uninstall_result_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  target_link_libraries(uninstall_result_test PRIVATE shell32)
  if(MINGW)
    target_link_options(uninstall_result_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME uninstall_result_reporting COMMAND uninstall_result_test)
  set_tests_properties(uninstall_result_reporting PROPERTIES TIMEOUT 10)
  # 2026-10-10 修复计划 T3：进程处置注入缝测试（杀序/形态分叉/假等待/名单）。
  add_executable(process_stop_order_test tests/process_stop_order_test.cpp)
  target_include_directories(process_stop_order_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  if(MINGW)
    target_link_options(process_stop_order_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME installer_process_stop_order COMMAND process_stop_order_test)
  set_tests_properties(installer_process_stop_order PROPERTIES TIMEOUT 10)
  # 2026-10-10 修复计划 T7/F7：解压输出锁有界重试（窗口内释放成功/耗尽失败）。
  # 耗尽用例固定消耗约 5 秒重试窗口，超时上限放宽到 20 秒。
  add_executable(archive_extract_retry_test tests/archive_extract_retry_test.cpp
    ${EXV_RUST_SETUP_PAYLOAD_SOURCES})
  target_include_directories(archive_extract_retry_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  target_link_libraries(archive_extract_retry_test PRIVATE cabinet)
  if(MINGW)
    target_link_options(archive_extract_retry_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME archive_extract_retry COMMAND archive_extract_retry_test)
  set_tests_properties(archive_extract_retry PROPERTIES TIMEOUT 20)
  add_executable(pfro_sweep_test tests/pfro_sweep_test.cpp pfro_sweep.cpp)
  target_include_directories(pfro_sweep_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  target_link_libraries(pfro_sweep_test PRIVATE advapi32)
  if(MINGW)
    target_link_options(pfro_sweep_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME installer_pfro_sweep_logic COMMAND pfro_sweep_test)
  set_tests_properties(installer_pfro_sweep_logic PROPERTIES TIMEOUT 10)
  add_executable(installer_pipe_test tests/installer_pipe_test.cpp pfro_sweep.cpp)
  target_include_directories(installer_pipe_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  target_link_libraries(installer_pipe_test PRIVATE shell32 advapi32)
  if(MINGW)
    target_link_options(installer_pipe_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME installer_pipe_timeout_and_recovery COMMAND installer_pipe_test)
  set_tests_properties(installer_pipe_timeout_and_recovery PROPERTIES TIMEOUT 10)
  # S5（2026-10-10 换盘服务迁移）：ImagePath 切分/比对规整纯函数单测——直编
  # 生产 service_control.cpp 真实现，只调纯函数零 SCM 访问（本机 ctest 未提权，
  # fixture 真服务迁移不可行；真机换盘迁移人工验收归 W5）。
  add_executable(service_image_path_test tests/service_image_path_test.cpp
    service_control.cpp util/hidden_process.cpp)
  target_include_directories(service_image_path_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  target_link_libraries(service_image_path_test PRIVATE advapi32)
  if(MINGW)
    target_link_options(service_image_path_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME installer_service_image_path_normalize COMMAND service_image_path_test)
  set_tests_properties(installer_service_image_path_normalize PROPERTIES TIMEOUT 10)
  # S6（2026-10-10 错误页重试，计划 §七 v2）：重试状态机单测——直编生产
  # ui/setup_window.cpp（守卫态/水面重置/排水转场真实实现），安装/卸载引擎与
  # 快捷方式等宿主副作用链接期整函数替换；不建窗口、不初始化 D2D。
  add_executable(install_retry_state_test tests/install_retry_state_test.cpp
    ui/icon_bitmaps.cpp progress_model.cpp)
  target_include_directories(install_retry_state_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  target_link_libraries(install_retry_state_test PRIVATE
    shell32 ole32 uuid user32 gdi32 d2d1 dwrite windowscodecs dwmapi msimg32)
  if(MINGW)
    target_link_options(install_retry_state_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME installer_install_retry_state COMMAND install_retry_state_test)
  set_tests_properties(installer_install_retry_state PROPERTIES TIMEOUT 30)
endif()

add_executable(pack_setup_payload
  tools/pack_setup_payload_main.cpp
  ${EXV_RUST_SETUP_PAYLOAD_SOURCES}
)
target_include_directories(pack_setup_payload PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
target_link_libraries(pack_setup_payload PRIVATE cabinet)
if(MINGW)
  target_link_options(pack_setup_payload PRIVATE
    -static
    -static-libgcc
    -static-libstdc++
  )
endif()
