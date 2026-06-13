#include "platform/win32/native_packet_device.hpp"

#include "utils.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <iphlpapi.h>

#include <cstring>
#include <limits>
#include <string>
#include <utility>

namespace ecnuvpn {
namespace platform {
namespace {

using AdapterHandle = NativeWintunApi::AdapterHandle;
using SessionHandle = NativeWintunApi::SessionHandle;

using WintunOpenAdapterFn = AdapterHandle(WINAPI *)(const wchar_t *);
using WintunCreateAdapterFn =
    AdapterHandle(WINAPI *)(const wchar_t *, const wchar_t *, const GUID *);
using WintunCloseAdapterFn = void(WINAPI *)(AdapterHandle);
using WintunGetAdapterLuidFn = void(WINAPI *)(AdapterHandle, NET_LUID *);
using WintunStartSessionFn = SessionHandle(WINAPI *)(AdapterHandle, DWORD);
using WintunEndSessionFn = void(WINAPI *)(SessionHandle);
using WintunReceivePacketFn = BYTE *(WINAPI *)(SessionHandle, DWORD *);
using WintunReleaseReceivePacketFn = void(WINAPI *)(SessionHandle, BYTE *);
using WintunAllocateSendPacketFn = BYTE *(WINAPI *)(SessionHandle, DWORD);
using WintunSendPacketFn = void(WINAPI *)(SessionHandle, BYTE *);

#include "platform/win32/native_packet_device_wintun_api.inc.cpp"
#include "platform/win32/native_packet_device_sessions.inc.cpp"
#include "platform/win32/native_packet_device_errors.inc.cpp"
#include "platform/win32/native_packet_device_public.inc.cpp"

} // namespace platform
} // namespace ecnuvpn
