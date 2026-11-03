"""Measure each synthetic AVIF size in a separate Windows Release process.

Includes the full input allocation in sampled working set; the Rust allocator
reports each codec stage's extra heap separately. These are measurements, not
an exemption from the production streaming budgets or an AVIF enable switch.
"""
import argparse
import ctypes
from ctypes import wintypes
import json
from pathlib import Path
import subprocess
import time


class ProcessMemory(ctypes.Structure):
    _fields_ = [("cb", wintypes.DWORD), ("page_faults", wintypes.DWORD)] + [
        (name, ctypes.c_size_t) for name in ["peak_working_set", "working_set", "peak_paged_pool",
        "paged_pool", "peak_nonpaged_pool", "nonpaged_pool", "pagefile", "peak_pagefile"]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--grid", action="store_true", help="measure the bounded qualification grid, retaining the same memory limits")
    modes.add_argument("--bands", action="store_true", help="measure real BandSource and SCPX row reads, retaining the same memory limits")
    parser.add_argument("--enforce-streaming-budget", action="store_true",
                        help="exit 1 if either size exceeds the existing 32 MiB extra heap / 16 MiB single allocation limits")
    args = parser.parse_args()
    mode = "bands" if args.bands else "grid" if args.grid else "full-frame"
    args.output = args.output or Path("target/hdr-avif-bands-resources" if args.bands else
                                     "target/hdr-avif-grid-resources" if args.grid else "target/hdr-avif-resources")
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.K32GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(ProcessMemory), wintypes.DWORD]
    kernel.K32GetProcessMemoryInfo.restype = wintypes.BOOL
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.CloseHandle.restype = wintypes.BOOL
    args.output.mkdir(parents=True, exist_ok=True)
    reports = []
    for width, height in [(3840, 2160), (8256, 5504)]:
        path = args.output / f"{width}x{height}.log"
        with path.open("w", encoding="utf8") as output:
            start = time.perf_counter()
            flag = "--band-resources" if args.bands else "--grid-resources" if args.grid else "--resources"
            child = subprocess.Popen([str(args.probe.resolve()), flag, str(width), str(height)],
                                     stdout=output, stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW)
            handle = kernel.OpenProcess(0x1000 | 0x10, False, child.pid)
            if not handle:
                child.kill()
                child.wait()
                raise ctypes.WinError(ctypes.get_last_error())
            peak = 0
            try:
                while child.poll() is None:
                    memory = ProcessMemory(cb=ctypes.sizeof(ProcessMemory))
                    if not kernel.K32GetProcessMemoryInfo(handle, ctypes.byref(memory), memory.cb):
                        if child.poll() is None:
                            child.kill()
                            child.wait()
                            raise ctypes.WinError(ctypes.get_last_error())
                        break
                    peak = max(peak, memory.peak_working_set, memory.working_set)
                    time.sleep(0.005)
            finally:
                kernel.CloseHandle(handle)
        log = path.read_text(encoding="utf8")
        stages = {}
        for line in log.splitlines():
            if line.startswith(("resource_encode ", "resource_decode ", "resource_cancel ", "resource_master_write ", "resource_master_encode ")):
                values = dict(token.split("=", 1) for token in line.split()[1:])
                stages[line.split()[0] + ("_" + values["operation"] if "operation" in values else "")] = values
        required = ["resource_encode", "resource_decode"]
        if args.bands:
            required += ["resource_master_write", "resource_master_encode"]
        fits = (all(name in stages for name in required)
                and all(int(stages[name]["heap_peak_delta"]) < 32 * 1024 * 1024
                        and int(stages[name]["largest_allocation"]) < 16 * 1024 * 1024
                        for name in required))
        report = dict(width=width, height=height, mode=mode, process_ms=(time.perf_counter() - start) * 1000,
                      peak_working_set=peak, stages=stages, fits_streaming_budget=fits, exit_code=child.returncode)
        reports.append(report)
        print(log, end="", flush=True)
        print(json.dumps(report), flush=True)
        (args.output / "summary.json").write_text(json.dumps(reports, indent=2) + "\n", encoding="utf8")
        if child.returncode:
            return child.returncode
    if args.enforce_streaming_budget and not all(report["fits_streaming_budget"] for report in reports):
        print("FAILED existing streaming memory budget; these measurements do not authorize production AVIF.", flush=True)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
