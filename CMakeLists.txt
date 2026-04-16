cmake_minimum_required(VERSION 3.15)
project(exv VERSION 1.0.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# Header-only nlohmann/json
include_directories(${CMAKE_SOURCE_DIR}/include)

# Source files
set(SOURCES
    src/main.cpp
    src/config.cpp
    src/crypto.cpp
    src/vpn.cpp
    src/tunnel.cpp
    src/logger.cpp
    src/utils.cpp
)

add_executable(exv ${SOURCES})

# Pass version to source
target_compile_definitions(exv PRIVATE
    ECNUVPN_VERSION="${PROJECT_VERSION}"
)

# Install target
install(TARGETS exv DESTINATION /usr/local/bin)
