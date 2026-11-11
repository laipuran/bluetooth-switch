#!/usr/bin/env python3
"""回归：忘记槽位 2 必须完成，且不能断开或删除仍在线的槽位 1。"""

import argparse
import json
import subprocess
import time
from pathlib import Path

from serial_client import SerialClient


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", default="/dev/ttyUSB0")
    parser.add_argument("--address", required=True)
    parser.add_argument("--log", required=True)
    args = parser.parse_args()
    serial = SerialClient(args.port, args.log + ".serial.log")
    result = {"passed": False}
    try:
        result["initial"] = serial.exchange("s")
        if "online=2" not in result["initial"]:
            raise RuntimeError("前置条件：两台已配对主机须同时在线")
        if not all(f"slot=Some({slot}) encrypted=true" in result["initial"] for slot in (1, 2)):
            raise RuntimeError("前置条件：两条连接必须已授权并加密，不能把握手中的连接算作就绪")
        serial.exchange("2")
        started = serial.write("X")
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            state = serial.exchange("s", 0.4)
            result["last_state"] = state
            if "2: empty" in state and "online=1" in state:
                if "slot=Some(1) encrypted=true" not in state:
                    raise RuntimeError("忘记槽位 2 时槽位 1 丢失")
                connected = subprocess.check_output([
                    "busctl", "--system", "get-property", "org.bluez",
                    "/org/bluez/hci0/dev_" + args.address.upper().replace(":", "_"),
                    "org.bluez.Device1", "Connected",
                ], text=True).strip()
                if connected != "b true":
                    raise RuntimeError("第一台主机实际连接丢失")
                result["completion_ms"] = (time.monotonic() - started) * 1000
                result["passed"] = True
                break
        if not result["passed"]:
            raise RuntimeError("8 秒内未完成忘记槽位 2，详见串口错误")
    except Exception as error:
        result["error"] = str(error)
    finally:
        serial.exchange("1")
        serial.close()
        Path(args.log).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(result, ensure_ascii=False, indent=2))
    if not result["passed"]:
        raise SystemExit(1)
    print("PASS: 仅忘记槽位 2，槽位 1 保持加密在线")


if __name__ == "__main__":
    main()
