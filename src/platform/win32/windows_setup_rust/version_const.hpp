#pragma once

// T8（F12）：安装器产品版本单一事实来源。
// GUI 安装（ui/setup_window.cpp）与静默安装（main.cpp）的 app_version 均取自
// EXV_PRODUCT_VERSION，最终写入 ARP Uninstall 键的 DisplayVersion
// （registry_install.cpp），两路径同源。
// 优先使用 CMake 注入的定义（CMakeLists.txt 的 EXV_PRODUCT_VERSION_DEF，经
// target_compile_definitions(exv-setup PRIVATE EXV_PRODUCT_VERSION=L"x.y.z")）；
// CMake 从根 release.json 注入版本，禁止独立维护默认版本。
#ifndef EXV_PRODUCT_VERSION
#error EXV_PRODUCT_VERSION must be provided by the release.json based CMake configuration
#endif
