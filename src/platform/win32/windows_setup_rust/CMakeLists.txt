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

set(EXV_PRODUCT_VERSION "4.0.0" CACHE STRING "EXV product version baked into the installer registry entry")

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
  uninstall_engine.cpp
  payload/embedded_payload.cpp
  ui/setup_window.cpp
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
  add_executable(setup_cli_options_test p1_setup_regression_test.cpp cli.cpp app_paths.cpp)
  target_include_directories(setup_cli_options_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  target_link_libraries(setup_cli_options_test PRIVATE shell32 ole32)
  if(MINGW)
    target_link_options(setup_cli_options_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME setup_cli_elevation_context COMMAND setup_cli_options_test)
  set_tests_properties(setup_cli_elevation_context PROPERTIES TIMEOUT 10)
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
  add_executable(installer_pipe_test tests/installer_pipe_test.cpp)
  target_include_directories(installer_pipe_test PRIVATE ${EXV_RUST_SETUP_INCLUDE_ROOT})
  target_link_libraries(installer_pipe_test PRIVATE shell32 advapi32)
  if(MINGW)
    target_link_options(installer_pipe_test PRIVATE -static -static-libgcc -static-libstdc++)
  endif()
  add_test(NAME installer_pipe_timeout_and_recovery COMMAND installer_pipe_test)
  set_tests_properties(installer_pipe_timeout_and_recovery PROPERTIES TIMEOUT 10)
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
