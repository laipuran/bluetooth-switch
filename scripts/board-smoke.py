#!/usr/bin/env python3
"""串口上板冒烟测试：查询状态、离线切槽、恢复原槽位，不忘记或配对主机。"""

import argparse
import os
import re
import select
import termios
import time
from pathlib import Path


def collect(fd, seconds, log):
    deadline = time.monotonic() + seconds
    chunks = []
    while time.monotonic() < deadline:
        readable, _, _ = select.select([fd], [], [], max(0, deadline - time.monotonic()))
        if readable:
            try:
                chunk = os.read(fd, 8192)
            except BlockingIOError:
                continue
            if not chunk:
                raise RuntimeError("串口已断开")
            chunks.append(chunk)
            log.write(chunk)
            log.flush()

    return b"".join(chunks).decode("utf-8", errors="replace")


def command(fd, value, log):
    os.write(fd, value.encode("ascii"))
    return collect(fd, 2, log)


def require(condition, message):
    if not condition:
        raise RuntimeError(message)
    print("PASS:", message)


def run(fd, log):
    # 等待可能发生的启动；不主动切换 DTR/RTS，也不进入下载模式。
    output = collect(fd, 8, log)
    output += command(fd, "h", log)
    status = command(fd, "s", log)
    output += status
    require("BT Switch (Rust) controls" in output, "Rust 固件响应帮助命令")
    match = re.search(r"slot (\d+)/(\d+) (?:connected=(true|false)|online=(\d+))", status)
    require(match is not None, "状态命令响应且包含槽位与连接状态")
    initial, count, connected, online = match.groups()

    if connected == "true" or (online is not None and int(online) > 0):
        print("SKIP: 已有 BLE 连接，不改变当前输入目标")
    else:
        target = str(int(initial) % int(count) + 1)
        try:
            output += command(fd, target, log)
            changed = command(fd, "s", log)
            output += changed
            require(f"slot {target}/{count} connected=false" in changed or f"slot {target}/{count} online=0" in changed, "离线切槽成功")
        finally:
            # 即使断言失败，也尝试恢复用户原先选择的槽位。
            output += command(fd, initial, log)

        restored = command(fd, "s", log)
        output += restored
        require(f"slot {initial}/{count}" in restored, "已恢复原槽位")

    output += collect(fd, 10, log)
    require(not re.search(r"Guru Meditation|panic|abort\(\)|Stack overflow", output, re.I), "观察期间未发现 panic 或崩溃日志")
    print("USB/BLE 端到端输入和多主机同时在线不在本脚本验证范围内。")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", default="/dev/ttyUSB0")
    parser.add_argument("--log", required=True)
    args = parser.parse_args()
    fd = os.open(args.port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    original = termios.tcgetattr(fd)
    try:
        config = termios.tcgetattr(fd)
        config[0] = 0
        config[1] = 0
        config[2] = termios.CS8 | termios.CLOCAL | termios.CREAD
        config[3] = 0
        config[4] = termios.B115200
        config[5] = termios.B115200
        config[6][termios.VMIN] = 0
        config[6][termios.VTIME] = 0
        termios.tcsetattr(fd, termios.TCSANOW, config)
        with Path(args.log).open("wb") as log:
            run(fd, log)
    finally:
        termios.tcsetattr(fd, termios.TCSANOW, original)
        os.close(fd)


if __name__ == "__main__":
    main()
