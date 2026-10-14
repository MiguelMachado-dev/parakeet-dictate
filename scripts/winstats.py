"""Read a process's resource counters on Windows: memory, CPU time, threads, loaded modules, GPU memory.

GPU memory comes from the WDDM "GPU Process Memory" performance counters, the per-process numbers Task
Manager shows. nvidia-smi reports N/A per process under WDDM.
"""

import ctypes
import os
from ctypes import wintypes

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
psapi = ctypes.WinDLL("psapi", use_last_error=True)
pdh = ctypes.WinDLL("pdh")

PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ = 0x0010
TH32CS_SNAPPROCESS = 0x2
TH32CS_SNAPTHREAD = 0x4
LIST_MODULES_ALL = 0x3
PDH_FMT_LARGE = 0x00000400
PDH_MORE_DATA = 0x800007D2
MB = 1 << 20


class PROCESS_MEMORY_COUNTERS_EX2(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("PageFaultCount", wintypes.DWORD),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
        ("PrivateUsage", ctypes.c_size_t),
        ("PrivateWorkingSetSize", ctypes.c_size_t),
        ("SharedCommitUsage", ctypes.c_ulonglong),
    ]


class THREADENTRY32(ctypes.Structure):
    _fields_ = [
        ("dwSize", wintypes.DWORD),
        ("cntUsage", wintypes.DWORD),
        ("th32ThreadID", wintypes.DWORD),
        ("th32OwnerProcessID", wintypes.DWORD),
        ("tpBasePri", wintypes.LONG),
        ("tpDeltaPri", wintypes.LONG),
        ("dwFlags", wintypes.DWORD),
    ]


class PROCESSENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", wintypes.DWORD),
        ("cntUsage", wintypes.DWORD),
        ("th32ProcessID", wintypes.DWORD),
        ("th32DefaultHeapID", ctypes.c_size_t),
        ("th32ModuleID", wintypes.DWORD),
        ("cntThreads", wintypes.DWORD),
        ("th32ParentProcessID", wintypes.DWORD),
        ("pcPriClassBase", wintypes.LONG),
        ("dwFlags", wintypes.DWORD),
        ("szExeFile", wintypes.WCHAR * 260),
    ]


class PDH_FMT_COUNTERVALUE(ctypes.Structure):
    _fields_ = [("CStatus", wintypes.DWORD), ("largeValue", ctypes.c_longlong)]


class PDH_FMT_COUNTERVALUE_ITEM_W(ctypes.Structure):
    _fields_ = [("szName", wintypes.LPWSTR), ("FmtValue", PDH_FMT_COUNTERVALUE)]


kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
kernel32.OpenProcess.restype = wintypes.HANDLE
kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
kernel32.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
kernel32.GetSystemTimes.argtypes = [ctypes.POINTER(wintypes.FILETIME)] * 3
kernel32.GetSystemTimePreciseAsFileTime.argtypes = [ctypes.POINTER(wintypes.FILETIME)]
kernel32.GetProcessHandleCount.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
kernel32.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
kernel32.Thread32First.argtypes = [wintypes.HANDLE, ctypes.POINTER(THREADENTRY32)]
kernel32.Thread32Next.argtypes = [wintypes.HANDLE, ctypes.POINTER(THREADENTRY32)]
kernel32.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESSENTRY32W)]
kernel32.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESSENTRY32W)]
psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD]
psapi.EnumProcessModulesEx.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.HMODULE), wintypes.DWORD, ctypes.POINTER(wintypes.DWORD), wintypes.DWORD]
psapi.GetModuleFileNameExW.argtypes = [wintypes.HANDLE, wintypes.HMODULE, wintypes.LPWSTR, wintypes.DWORD]
pdh.PdhOpenQueryW.argtypes = [wintypes.LPCWSTR, ctypes.c_size_t, ctypes.POINTER(wintypes.HANDLE)]
pdh.PdhAddEnglishCounterW.argtypes = [wintypes.HANDLE, wintypes.LPCWSTR, ctypes.c_size_t, ctypes.POINTER(wintypes.HANDLE)]
pdh.PdhCollectQueryData.argtypes = [wintypes.HANDLE]
pdh.PdhGetFormattedCounterArrayW.argtypes = [
    wintypes.HANDLE, wintypes.DWORD, ctypes.POINTER(wintypes.DWORD), ctypes.POINTER(wintypes.DWORD), ctypes.c_void_p,
]
pdh.PdhCloseQuery.argtypes = [wintypes.HANDLE]
for _function in (pdh.PdhOpenQueryW, pdh.PdhAddEnglishCounterW, pdh.PdhCollectQueryData, pdh.PdhGetFormattedCounterArrayW):
    _function.restype = wintypes.DWORD


def _filetime(value: wintypes.FILETIME) -> int:
    return value.dwHighDateTime << 32 | value.dwLowDateTime


def _open(pid: int, access: int = PROCESS_QUERY_LIMITED_INFORMATION) -> int:
    handle = kernel32.OpenProcess(access, False, pid)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    return handle


def memory(pid: int | None = None) -> dict[str, float]:
    """Memory in MiB and CPU time in seconds. commit is private bytes (PrivateUsage): memory the process
    has committed, resident or not. working_set is what it has resident in RAM right now."""
    handle = _open(pid or os.getpid())
    try:
        counters = PROCESS_MEMORY_COUNTERS_EX2(cb=ctypes.sizeof(PROCESS_MEMORY_COUNTERS_EX2))
        if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        times = [wintypes.FILETIME() for _ in range(4)]
        kernel32.GetProcessTimes(handle, *map(ctypes.byref, times))
        handles = wintypes.DWORD()
        kernel32.GetProcessHandleCount(handle, ctypes.byref(handles))
    finally:
        kernel32.CloseHandle(handle)
    return {
        "working_set_mb": counters.WorkingSetSize / MB,
        "peak_working_set_mb": counters.PeakWorkingSetSize / MB,
        "private_working_set_mb": counters.PrivateWorkingSetSize / MB,
        "commit_mb": counters.PrivateUsage / MB,
        "peak_commit_mb": counters.PeakPagefileUsage / MB,
        "cpu_s": (_filetime(times[2]) + _filetime(times[3])) / 1e7,
        "handles": handles.value,
    }


def seconds_since_start(pid: int | None = None) -> float:
    """Wall seconds since the process was created, by the system clock."""
    handle = _open(pid or os.getpid())
    try:
        times = [wintypes.FILETIME() for _ in range(4)]
        kernel32.GetProcessTimes(handle, *map(ctypes.byref, times))
    finally:
        kernel32.CloseHandle(handle)
    now = wintypes.FILETIME()
    kernel32.GetSystemTimePreciseAsFileTime(ctypes.byref(now))
    return (_filetime(now) - _filetime(times[0])) / 1e7


def threads(pid: int | None = None) -> int:
    pid = pid or os.getpid()
    snapshot = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
    entry = THREADENTRY32(dwSize=ctypes.sizeof(THREADENTRY32))
    count = 0
    try:
        ok = kernel32.Thread32First(snapshot, ctypes.byref(entry))
        while ok:
            count += entry.th32OwnerProcessID == pid
            ok = kernel32.Thread32Next(snapshot, ctypes.byref(entry))
    finally:
        kernel32.CloseHandle(snapshot)
    return count


def child_pids(pid: int) -> list[int]:
    """A venv's python.exe and pythonw.exe are launchers that run the base interpreter as a child process."""
    snapshot = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    entry = PROCESSENTRY32W(dwSize=ctypes.sizeof(PROCESSENTRY32W))
    found = []
    try:
        ok = kernel32.Process32FirstW(snapshot, ctypes.byref(entry))
        while ok:
            if entry.th32ParentProcessID == pid:
                found.append(entry.th32ProcessID)
            ok = kernel32.Process32NextW(snapshot, ctypes.byref(entry))
    finally:
        kernel32.CloseHandle(snapshot)
    return found


def modules(pid: int) -> list[str]:
    """File names of the DLLs and extension modules loaded in the process."""
    handle = _open(pid, PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)
    try:
        needed = wintypes.DWORD()
        psapi.EnumProcessModulesEx(handle, None, 0, ctypes.byref(needed), LIST_MODULES_ALL)
        array = (wintypes.HMODULE * (needed.value // ctypes.sizeof(wintypes.HMODULE) + 64))()
        if not psapi.EnumProcessModulesEx(handle, array, ctypes.sizeof(array), ctypes.byref(needed), LIST_MODULES_ALL):
            raise ctypes.WinError(ctypes.get_last_error())
        names = []
        path = ctypes.create_unicode_buffer(32768)
        for module in array[: needed.value // ctypes.sizeof(wintypes.HMODULE)]:
            if psapi.GetModuleFileNameExW(handle, module, path, len(path)):
                names.append(os.path.basename(path.value))
        return sorted(names, key=str.casefold)
    finally:
        kernel32.CloseHandle(handle)


def system_cpu() -> tuple[int, int]:
    """(busy, total) 100 ns ticks over all cores since boot; diff two readings for the busy share."""
    idle, kernel, user = (wintypes.FILETIME() for _ in range(3))
    kernel32.GetSystemTimes(ctypes.byref(idle), ctypes.byref(kernel), ctypes.byref(user))
    total = _filetime(kernel) + _filetime(user)
    return total - _filetime(idle), total


class GpuMemory:
    """Dedicated and shared GPU memory per process, in MiB. Each read collects every process's counters,
    which takes tens to hundreds of milliseconds, so keep reads out of timed code."""

    def __init__(self) -> None:
        self._query = wintypes.HANDLE()
        if pdh.PdhOpenQueryW(None, 0, ctypes.byref(self._query)):
            raise OSError("PdhOpenQueryW failed")
        self._counters = {}
        for key, name in (("gpu_dedicated_mb", "Dedicated Usage"), ("gpu_shared_mb", "Shared Usage")):
            counter = wintypes.HANDLE()
            status = pdh.PdhAddEnglishCounterW(self._query, rf"\GPU Process Memory(*)\{name}", 0, ctypes.byref(counter))
            if status:
                raise OSError(f"PdhAddEnglishCounterW({name}) failed: {status:#x}")
            self._counters[key] = counter

    def read(self, pid: int | None = None) -> dict[str, float]:
        prefix = f"pid_{pid or os.getpid()}_"
        if status := pdh.PdhCollectQueryData(self._query):
            raise OSError(f"PdhCollectQueryData failed: {status:#x}")
        return {key: sum(value for name, value in self._items(counter) if name.startswith(prefix)) / MB
                for key, counter in self._counters.items()}

    @staticmethod
    def _items(counter: wintypes.HANDLE) -> list[tuple[str, int]]:
        # The instance list can grow between the size query and the read, so retry with the new size.
        for _ in range(10):
            size, count = wintypes.DWORD(0), wintypes.DWORD(0)
            status = pdh.PdhGetFormattedCounterArrayW(counter, PDH_FMT_LARGE, ctypes.byref(size), ctypes.byref(count), None)
            if status not in (0, PDH_MORE_DATA):
                raise OSError(f"PdhGetFormattedCounterArrayW failed: {status:#x}")
            size.value += 8192
            buffer = (ctypes.c_byte * size.value)()
            status = pdh.PdhGetFormattedCounterArrayW(counter, PDH_FMT_LARGE, ctypes.byref(size), ctypes.byref(count), buffer)
            if status == 0:
                items = ctypes.cast(buffer, ctypes.POINTER(PDH_FMT_COUNTERVALUE_ITEM_W))
                return [(items[i].szName, items[i].FmtValue.largeValue) for i in range(count.value)]
            if status != PDH_MORE_DATA:
                raise OSError(f"PdhGetFormattedCounterArrayW failed: {status:#x}")
        raise OSError("PdhGetFormattedCounterArrayW kept asking for a larger buffer")

    def close(self) -> None:
        pdh.PdhCloseQuery(self._query)
