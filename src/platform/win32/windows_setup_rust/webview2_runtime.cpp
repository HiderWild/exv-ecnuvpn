#include "windows_setup_rust/webview2_runtime.hpp"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <array>
#include <string_view>

namespace exv::setup {
namespace {

constexpr wchar_t kRuntimeClientsKey[] =
    L"SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
constexpr wchar_t kRuntimePvValue[] = L"pv";

bool HasRuntimeVersion(HKEY root, const wchar_t *subkey, REGSAM view) {
  HKEY key = nullptr;
  if (RegOpenKeyExW(root, subkey, 0, KEY_QUERY_VALUE | view, &key) != ERROR_SUCCESS) {
    return false;
  }

  std::array<wchar_t, 64> version{};
  DWORD type = 0;
  DWORD size = static_cast<DWORD>(sizeof(version));
  const LONG result = RegQueryValueExW(key, kRuntimePvValue, nullptr, &type,
                                       reinterpret_cast<LPBYTE>(version.data()), &size);
  RegCloseKey(key);
  if (result != ERROR_SUCCESS || type != REG_SZ || size < sizeof(wchar_t) ||
      size > sizeof(version) || size % sizeof(wchar_t) != 0) {
    return false;
  }

  const auto units = static_cast<std::size_t>(size / sizeof(wchar_t));
  if (version[units - 1] != L'\0') {
    return false;
  }
  return IsUsableWebView2RuntimeVersion(std::wstring_view(version.data(), units - 1));
}

}  // namespace

bool IsUsableWebView2RuntimeVersion(std::wstring_view version) {
  if (version.empty() || version.back() == L'.') {
    return false;
  }

  std::size_t part_count = 0;
  bool nonzero = false;
  std::size_t begin = 0;
  while (begin < version.size()) {
    const auto end = version.find(L'.', begin);
    const auto part_end = end == std::wstring_view::npos ? version.size() : end;
    if (part_end == begin) {
      return false;
    }
    for (std::size_t i = begin; i < part_end; ++i) {
      if (version[i] < L'0' || version[i] > L'9') {
        return false;
      }
      nonzero = nonzero || version[i] != L'0';
    }
    ++part_count;
    if (end == std::wstring_view::npos) {
      begin = version.size();
    } else {
      begin = end + 1;
    }
  }
  return part_count == 4 && nonzero;
}

bool IsWebView2EvergreenRuntimeInstalled() {
  // Microsoft documents the 32-bit EdgeUpdate client registry view for the
  // per-machine runtime on 64-bit Windows; HKCU covers a per-user install.
  return HasRuntimeVersion(HKEY_LOCAL_MACHINE, kRuntimeClientsKey, KEY_WOW64_32KEY) ||
         HasRuntimeVersion(HKEY_CURRENT_USER, kRuntimeClientsKey, 0);
}

}  // namespace exv::setup
