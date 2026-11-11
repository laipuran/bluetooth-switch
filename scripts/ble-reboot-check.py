#!/usr/bin/env python3
"""实板重启后双主机加密恢复检查；不擦除 Flash、不删除或重新配对。"""

import argparse
import concurrent.futures
import json
import shlex
import subprocess
import time
from pathlib import Path

from serial_client import SerialClient


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--remote", required=True)
    parser.add_argument("--ssh-command", default="ssh -o BatchMode=yes -o ConnectTimeout=8")
    parser.add_argument("--address", required=True)
    parser.add_argument("--port", default="/dev/ttyUSB0")
    parser.add_argument("--log", required=True)
    parser.add_argument("--max-recovery-seconds", type=float, default=10, help="调试门槛，默认 10 秒，不把 30 秒认证超时后的重连算作快速恢复")
    args = parser.parse_args()
    serial = SerialClient(args.port, args.log + ".serial.log")
    path = "/org/bluez/hci0/dev_" + args.address.upper().replace(":", "_")
    connect = ["busctl", "--system", "--timeout=20s", "call", "org.bluez", path, "org.bluez.Device1", "Connect"]
    result = {"passed": False}

    def request(command):
        completed = subprocess.run(command, text=True, capture_output=True, timeout=25)
        return {"returncode": completed.returncode, "stderr": completed.stderr.strip()}

    try:
        result["initial"] = serial.exchange("s")
        if "online=2" not in result["initial"]:
            raise RuntimeError("前置条件：两台主机须在线")
        if not all(f"slot=Some({slot}) encrypted=true" in result["initial"] for slot in (1, 2)):
            raise RuntimeError("前置条件：两台主机须已完成加密授权，不能从正在握手的状态复位")
        reset = subprocess.run(["espflash", "reset", "--port", args.port, "--chip", "esp32s3", "--non-interactive", "--skip-update-check"],
                               text=True, capture_output=True, timeout=20, check=True)
        result["reset_output"] = reset.stdout + reset.stderr
        serial.collect(1)
        started = time.monotonic()
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            jobs = [pool.submit(request, connect), pool.submit(request, [*shlex.split(args.ssh_command), args.remote, shlex.join(connect)])]
            deadline = started + args.max_recovery_seconds
            while time.monotonic() < deadline:
                status = serial.exchange("s", 0.5)
                result["last_state"] = status
                if "online=2" in status and all(f"slot=Some({slot}) encrypted=true" in status for slot in (1, 2)):
                    result["recovery_ms"] = (time.monotonic() - started) * 1000
                    result["passed"] = True
                    break
            result["connect_requests"] = [job.result() for job in jobs]
        if not result["passed"]:
            raise RuntimeError(f"{args.max_recovery_seconds} 秒内未恢复两条加密授权连接，详见串口日志")
    except Exception as error:
        result["error"] = str(error)
    finally:
        serial.close()
        Path(args.log).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(result, ensure_ascii=False, indent=2))
    if not result["passed"]:
        raise SystemExit(1)
    print("PASS: 重启后未重新配对，两条加密连接恢复")


if __name__ == "__main__":
    main()
