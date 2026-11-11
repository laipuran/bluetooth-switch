#!/usr/bin/env python3
"""单主机验证多连接固件的离线目标隔离，不代替两机并发测试。"""

import argparse
import importlib.util
import json
import os
import re
import termios
from pathlib import Path

from observer_client import ObserverClient


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True)
    parser.add_argument("--port", default="/dev/ttyUSB0")
    parser.add_argument("--log", required=True)
    args = parser.parse_args()
    spec = importlib.util.spec_from_file_location("board_smoke", Path(__file__).with_name("board-smoke.py"))
    serial = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(serial)
    fd = os.open(args.port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    original = termios.tcgetattr(fd)
    config = termios.tcgetattr(fd)
    config[0] = 0
    config[1] = 0
    config[2] = termios.CS8 | termios.CLOCAL | termios.CREAD
    config[3] = 0
    config[4] = termios.B115200
    config[5] = termios.B115200
    termios.tcsetattr(fd, termios.TCSANOW, config)
    observer = None
    initial_slot = None
    result = {}

    try:
        with Path(args.log + ".serial.log").open("wb") as log:
            status = serial.command(fd, "s", log)
            match = re.search(r"slot (\d+)/(\d+) online=1", status)
            if not match:
                raise RuntimeError("测试要求恰好一条连接，且固件使用多连接状态输出")
            initial_slot = match[1]
            empty = re.search(r"\s(\d+): empty", status)
            if not empty:
                raise RuntimeError("没有可用于离线目标检查的空槽位")

            observer = ObserverClient(["/usr/bin/python3", "-u", str(Path(__file__).with_name("ble-observer.py")), "--address", args.address])
            observer.request("subscribe")
            serial.collect(fd, 0.5, log)

            def probe():
                observer.request("reset")
                os.write(fd, b"t")
                serial.collect(fd, 0.5, log)
                return observer.request("status")

            baseline = probe()
            expected = {("report1", 8, True), ("report2", 7, True), ("report3", 2, True)}
            actual = {(item["report"], item["length"], item["neutral"]) for item in baseline["notifications"]}
            if actual != expected:
                raise RuntimeError(f"目标主机未收到完整全零探针: {baseline}")
            result["neutral_probe"] = True

            serial.command(fd, empty[1], log)
            status = serial.command(fd, "s", log)
            offline = probe()
            if f"slot {empty[1]}/{match[2]} online=1 ready=false" not in status:
                raise RuntimeError("离线目标未暂停输入或原连接丢失")
            if not offline["connected"] or offline["notifications"]:
                raise RuntimeError(f"离线目标错误地回退输入或断开连接: {offline}")
            result["offline_target_preserves_connection_without_fallback"] = True

            serial.command(fd, initial_slot, log)
            restored = probe()
            actual = {(item["report"], item["length"], item["neutral"]) for item in restored["notifications"]}
            if not restored["connected"] or actual != expected:
                raise RuntimeError(f"切回原槽位后定向通知未恢复: {restored}")
            result["return_without_repairing"] = True
            result["simultaneous_hosts_tested"] = False
    finally:
        if initial_slot is not None:
            os.write(fd, initial_slot.encode("ascii"))
        if observer is not None:
            observer.close()
        termios.tcsetattr(fd, termios.TCSANOW, original)
        os.close(fd)
        Path(args.log).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")

    print(json.dumps(result, ensure_ascii=False, indent=2))
    print("PASS: 单主机通知与离线目标隔离通过；两机同时在线仍需独立验证")


if __name__ == "__main__":
    main()
