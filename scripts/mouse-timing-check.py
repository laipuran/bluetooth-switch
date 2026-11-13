#!/usr/bin/env python3
"""只读统计指定鼠标移动帧间隔；不独占输入、不保存坐标或按键。"""

import argparse
import json
import os
from pathlib import Path
import select
import struct
import time


def percentile(values, percent):
    ordered = sorted(values)
    return ordered[round((len(ordered) - 1) * percent / 100)] if ordered else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=float, default=20)
    parser.add_argument("--name", default="BT Switch Mouse")
    parser.add_argument("--max-p95-ms", type=float, default=10)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    devices = [p for p in Path("/sys/class/input").glob("event*")
               if (p / "device/name").read_text().strip() == args.name]
    if len(devices) != 1:
        raise SystemExit(f"需要唯一的 {args.name}，实际找到 {len(devices)} 个")
    device = "/dev/input/" + devices[0].name
    fd = os.open(device, os.O_RDONLY | os.O_NONBLOCK)
    event = struct.Struct("@llHHi")
    pending = b""
    moving = False
    timestamps = []
    deadline = time.monotonic() + args.seconds
    print(f"READY: {device}; 请持续移动鼠标 {args.seconds:g} 秒，不点击、不切换槽位", flush=True)
    try:
        while time.monotonic() < deadline:
            if not select.select([fd], [], [], max(0, deadline - time.monotonic()))[0]:
                break
            pending += os.read(fd, event.size * 256)
            while len(pending) >= event.size:
                seconds, micros, kind, code, value = event.unpack_from(pending)
                pending = pending[event.size:]
                if kind == 2 and code in (0, 1) and value:
                    moving = True
                elif kind == 0 and code == 3:
                    raise RuntimeError("SYN_DROPPED: 输入捕获溢出，本轮结果无效")
                elif kind == 0 and code == 0:
                    if moving:
                        timestamps.append(seconds * 1_000_000 + micros)
                    moving = False
    finally:
        os.close(fd)
    intervals = [(b - a) / 1000 for a, b in zip(timestamps, timestamps[1:])]
    span = (timestamps[-1] - timestamps[0]) / 1e6 if len(timestamps) > 1 else 0
    result = {
        "device": device,
        "name": args.name,
        "motion_frames": len(timestamps),
        "motion_span_seconds": span,
        "average_hz": len(intervals) / span if span else None,
        "interval_ms": {"p50": percentile(intervals, 50), "p95": percentile(intervals, 95),
                        "p99": percentile(intervals, 99), "max": max(intervals) if intervals else None},
        "interval_histogram_ms": {
            "<2": sum(x < 2 for x in intervals),
            "2..5": sum(2 <= x < 5 for x in intervals),
            "5..10": sum(5 <= x < 10 for x in intervals),
            "10..20": sum(10 <= x < 20 for x in intervals),
            "20..50": sum(20 <= x < 50 for x in intervals),
            ">=50": sum(x >= 50 for x in intervals),
        },
        "target_p95_ms": args.max_p95_ms,
        "note": "仅统计非零 XY 移动帧；含人为停顿；evdev 时间不是屏幕刷新时间或 USB 到 BLE 延迟",
    }
    result["verdict"] = ("INCONCLUSIVE" if len(intervals) < 100 else
                         "PASS" if result["interval_ms"]["p95"] <= args.max_p95_ms else "FAIL")
    Path(args.output).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(result, ensure_ascii=False, indent=2), flush=True)
    raise SystemExit(0 if result["verdict"] == "PASS" else 1)


if __name__ == "__main__":
    main()
