#!/usr/bin/env python3
"""只读检查 Windows 安装包的 EXVP01 资源、LZMS、CRC 和源文件摘要。

PE 资源读取及 LZMS 解压需要 Windows；parse_archive 和 Store 核验可跨平台运行。
所有载荷只在内存中解压，不运行安装器。--source-dir 应保持归档的相对目录结构。
"""

import argparse
import ctypes as c
from ctypes import wintypes as w
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
import zlib


def parse_archive(blob: bytes) -> dict:
    """解析索引及字节分项，不调用 Windows API，也不解压载荷。"""
    if len(blob) < 22 or blob[:6] != b"EXVP01":
        raise ValueError("EXVP01 header missing or truncated")
    flags, total, count = struct.unpack_from("<IQI", blob, 6)
    pos = 22
    files = []
    for _ in range(count):
        if pos + 2 > len(blob):
            raise ValueError("truncated EXVP01 index path length")
        name_size = struct.unpack_from("<H", blob, pos)[0]
        pos += 2
        if pos + name_size + 28 > len(blob):
            raise ValueError("truncated EXVP01 index entry")
        name = blob[pos:pos + name_size].decode("utf-8")
        pos += name_size
        offset, compressed, raw, crc = struct.unpack_from("<QQQI", blob, pos)
        pos += 28
        files.append(dict(name=name, offset=offset, compressed_size=compressed,
                          raw_size=raw, crc32=crc))
    for entry in files:
        if pos + entry["offset"] + entry["compressed_size"] > len(blob):
            raise ValueError(f"entry data out of range: {entry['name']}")
    return dict(flags=flags, uncompressed_total=total, file_count=count,
                compressed_total=sum(entry["compressed_size"] for entry in files),
                payload_bytes=len(blob), index_bytes=pos, data_region_begin=pos,
                files=files)


def read_payload_resource(installer: Path) -> bytes:
    """以 LOAD_LIBRARY_AS_DATAFILE 映射资源 401 / RT_RCDATA，不执行入口点。"""
    if os.name != "nt":
        raise OSError("PE resource inspection requires Windows")
    kernel = c.WinDLL("kernel32", use_last_error=True)
    kernel.LoadLibraryExW.argtypes = [w.LPCWSTR, w.HANDLE, w.DWORD]
    kernel.LoadLibraryExW.restype = w.HMODULE
    kernel.FindResourceW.argtypes = [w.HMODULE, c.c_void_p, c.c_void_p]
    kernel.FindResourceW.restype = w.HANDLE
    kernel.SizeofResource.argtypes = [w.HMODULE, w.HANDLE]
    kernel.SizeofResource.restype = w.DWORD
    kernel.LoadResource.argtypes = [w.HMODULE, w.HANDLE]
    kernel.LoadResource.restype = w.HANDLE
    kernel.LockResource.argtypes = [w.HANDLE]
    kernel.LockResource.restype = c.c_void_p
    kernel.FreeLibrary.argtypes = [w.HMODULE]
    kernel.FreeLibrary.restype = w.BOOL
    module = kernel.LoadLibraryExW(str(installer.resolve()), None, 2)
    if not module:
        raise c.WinError(c.get_last_error())
    try:
        resource = kernel.FindResourceW(module, 401, 10)
        if not resource:
            raise c.WinError(c.get_last_error())
        size = kernel.SizeofResource(module, resource)
        loaded = kernel.LoadResource(module, resource)
        if not loaded:
            raise c.WinError(c.get_last_error())
        pointer = kernel.LockResource(loaded)
        if not pointer:
            raise OSError("LockResource failed for EXVP01 payload")
        return c.string_at(pointer, size)
    finally:
        kernel.FreeLibrary(module)


def decompress_lzms(payload: bytes, raw_size: int) -> bytes:
    """沿用安装器的 Windows LZMS 算法及 512 MiB 单文件解压上限。"""
    if os.name != "nt":
        raise OSError("LZMS decompression requires Windows")
    if raw_size > 512 * 1024 * 1024:
        raise ValueError("entry exceeds installer decompression limit")
    cabinet = c.WinDLL("cabinet", use_last_error=True)
    cabinet.CreateDecompressor.argtypes = [w.DWORD, c.c_void_p, c.POINTER(c.c_void_p)]
    cabinet.CreateDecompressor.restype = w.BOOL
    cabinet.Decompress.argtypes = [c.c_void_p, c.c_void_p, c.c_size_t,
                                  c.c_void_p, c.c_size_t, c.POINTER(c.c_size_t)]
    cabinet.Decompress.restype = w.BOOL
    cabinet.CloseDecompressor.argtypes = [c.c_void_p]
    cabinet.CloseDecompressor.restype = w.BOOL
    handle = c.c_void_p()
    if not cabinet.CreateDecompressor(5, None, c.byref(handle)):
        raise c.WinError(c.get_last_error())
    try:
        target = c.create_string_buffer(raw_size)
        actual = c.c_size_t()
        if not cabinet.Decompress(handle, payload, len(payload), target, raw_size,
                                  c.byref(actual)):
            raise c.WinError(c.get_last_error())
        if actual.value != raw_size:
            raise ValueError("LZMS decompressed size mismatch")
        return target.raw[:actual.value]
    finally:
        cabinet.CloseDecompressor(handle)


def inspect_archive(blob: bytes, source_dir: Path | None = None) -> dict:
    """核验每项载荷；CRC 或源文件不匹配保留在报告中，并令 verified 为假。"""
    result = parse_archive(blob)
    algorithms = {0: "Store", 3: "WindowsLzms"}
    if result["flags"] not in algorithms:
        raise ValueError(f"unsupported compression algorithm: {result['flags']}")
    result["algorithm"] = algorithms[result["flags"]]
    result["verified"] = True
    for entry in result["files"]:
        start = result["data_region_begin"] + entry["offset"]
        payload = blob[start:start + entry["compressed_size"]]
        plain = (payload if result["flags"] == 0
                 else decompress_lzms(payload, entry["raw_size"]))
        if len(plain) != entry["raw_size"]:
            raise ValueError(f"entry raw size mismatch: {entry['name']}")
        entry["crc_verified"] = zlib.crc32(plain) == entry["crc32"]
        entry["sha256"] = hashlib.sha256(plain).hexdigest()
        entry["source_match"] = None
        if source_dir is not None:
            source = Path(source_dir) / entry["name"]
            try:
                source_bytes = source.read_bytes()
            except FileNotFoundError:
                entry["source_sha256"] = None
                entry["source_match"] = False
            else:
                entry["source_sha256"] = hashlib.sha256(source_bytes).hexdigest()
                entry["source_match"] = source_bytes == plain
        result["verified"] &= entry["crc_verified"] and entry["source_match"] is not False
    return result


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--installer", type=Path, required=True)
    parser.add_argument("--source-dir", type=Path)
    parser.add_argument("--json-output", type=Path)
    args = parser.parse_args(argv)
    try:
        installer = args.installer.resolve()
        installer_bytes = installer.read_bytes()
        result = inspect_archive(read_payload_resource(installer), args.source_dir)
        result.update(path=str(installer), bytes=len(installer_bytes),
                      sha256=hashlib.sha256(installer_bytes).hexdigest(),
                      stub_and_alignment=len(installer_bytes) - result["payload_bytes"],
                      source_dir=str(args.source_dir.resolve()) if args.source_dir else None)
        output = json.dumps(result, ensure_ascii=False, indent=2) + "\n"
        if args.json_output:
            args.json_output.write_text(output, encoding="utf-8")
        print(output, end="")
        return 0 if result["verified"] else 1
    except (OSError, ValueError) as error:
        print(f"安装包检查失败：{error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
