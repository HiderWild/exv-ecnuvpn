"""使用 macOS 原生 API 写入并验证 Finder 背景书签。"""
import ctypes as C
from pathlib import Path

from ds_store import DSStore, DSStoreEntry
import ds_store.store


class NativeBookmark:
    def __init__(self):
        self.cf = C.CDLL('/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation')
        p = C.c_void_p
        signatures = {
            'CFDataCreate': ([p, p, C.c_long], p),
            'CFDataGetLength': ([p], C.c_long),
            'CFDataGetBytePtr': ([p], p),
            'CFURLCreateFromFileSystemRepresentation': ([p, p, C.c_long, C.c_ubyte], p),
            'CFURLCreateBookmarkData': ([p, p, C.c_ulong, p, p, C.POINTER(p)], p),
            'CFURLCreateByResolvingBookmarkData': ([p, p, C.c_ulong, p, p, C.POINTER(C.c_ubyte), C.POINTER(p)], p),
            'CFURLCopyFileSystemPath': ([p, C.c_int], p),
            'CFErrorCopyDescription': ([p], p),
            'CFStringGetCString': ([p, p, C.c_long, C.c_uint], C.c_ubyte),
            'CFRelease': ([p], None),
        }
        for name, (args, result) in signatures.items():
            fn = getattr(self.cf, name)
            fn.argtypes, fn.restype = args, result

    def string(self, obj):
        buf = C.create_string_buffer(8192)
        if not self.cf.CFStringGetCString(obj, buf, len(buf), 0x08000100):
            raise RuntimeError('无法读取原生书签路径')
        return buf.value.decode('utf-8')

    def failure(self, error):
        if not error.value:
            return RuntimeError('原生书签操作失败')
        message = self.cf.CFErrorCopyDescription(error)
        try:
            return RuntimeError(self.string(message))
        finally:
            self.cf.CFRelease(message)
            self.cf.CFRelease(error)

    def create(self, path):
        encoded = str(path).encode('utf-8')
        url = self.cf.CFURLCreateFromFileSystemRepresentation(None, encoded, len(encoded), False)
        error = C.c_void_p()
        data = self.cf.CFURLCreateBookmarkData(None, url, 0, None, None, C.byref(error))
        self.cf.CFRelease(url)
        if not data:
            raise self.failure(error)
        try:
            return C.string_at(self.cf.CFDataGetBytePtr(data), self.cf.CFDataGetLength(data))
        finally:
            self.cf.CFRelease(data)

    def resolve(self, payload):
        data = self.cf.CFDataCreate(None, payload, len(payload))
        stale, error = C.c_ubyte(), C.c_void_p()
        # 不弹 UI、不自动挂载其他卷；验证当前已挂载镜像内的真实文件。
        url = self.cf.CFURLCreateByResolvingBookmarkData(None, data, 0x300, None, None, C.byref(stale), C.byref(error))
        self.cf.CFRelease(data)
        if not url:
            raise self.failure(error)
        text = self.cf.CFURLCopyFileSystemPath(url, 0)
        try:
            return Path(self.string(text))
        finally:
            self.cf.CFRelease(text)
            self.cf.CFRelease(url)


def read_raw_bookmark(ds_path):
    # 不经 mac_alias 重新序列化，否则验证对象可能与磁盘字节不同。
    codec = ds_store.store.codecs.pop(b'pBBk', None)
    try:
        with DSStore.open(str(ds_path), 'r') as store:
            value = store['.']['pBBk']
            return bytes(value[1] if isinstance(value, tuple) else value)
    finally:
        if codec is not None:
            ds_store.store.codecs[b'pBBk'] = codec


def verify_background(mount):
    mount = Path(mount)
    actual = NativeBookmark().resolve(read_raw_bookmark(mount / '.DS_Store'))
    expected = mount / '.background.png'
    if actual != expected or not actual.is_file():
        raise RuntimeError(f'背景书签未解析到当前镜像：{actual}，期望 {expected}')
    print(f'[background] 原生书签解析通过：{actual}')


def repair_background(mount):
    mount = Path(mount)
    payload = NativeBookmark().create(mount / '.background.png')
    with DSStore.open(str(mount / '.DS_Store'), 'r+') as store:
        store.insert(DSStoreEntry('.', b'pBBk', b'blob', payload))
    verify_background(mount)
