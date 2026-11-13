#!/usr/bin/env python3
"""实板 USB 识别回归：只查询状态与缓存描述符，不读写存储介质。"""

import argparse
import json
import re
import time
from pathlib import Path

from serial_client import SerialClient


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", default="/dev/ttyUSB0")
    parser.add_argument("--timeout", type=float, default=10)
    parser.add_argument("--log", required=True)
    args = parser.parse_args()
    serial = SerialClient(args.port, args.log + ".serial.log")
    result = {"identified": False, "devices": []}
    try:
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            text = serial.exchange("s", 1)
            identities = re.findall(r"USB device address=(\d+) VID=([0-9a-f]+) PID=([0-9a-f]+).*?class=([0-9a-f]+)", text)
            if identities:
                result["devices"] = [{"address": int(addr), "vid": vid, "pid": pid, "class": kind} for addr, vid, pid, kind in identities]
                result["identified"] = True
                break
            result["last_usb_status"] = [line for line in text.splitlines() if "USB Host:" in line or "USB port:" in line or "USB PHY:" in line]
    finally:
        serial.close()
        Path(args.log).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(result, ensure_ascii=False, indent=2))
    if not result["identified"]:
        raise SystemExit("FAIL: 未取得任何 USB 设备的 VID/PID，详见控制器与 PHY 状态")
    print("PASS: 通用 USB 设备已识别，不代表接口驱动或输入链路已验证")


if __name__ == "__main__":
    main()
