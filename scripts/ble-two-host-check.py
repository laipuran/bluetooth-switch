#!/usr/bin/env python3
"""两机通知隔离回归：同时在线、切换释放、定向探针及离线目标，不合成输入。"""

import argparse
import json
import re
import shlex
import subprocess
import time
from pathlib import Path

from observer_client import ObserverClient
from serial_client import SerialClient

EXPECTED = {("report1", 8), ("report2", 7), ("report3", 2)}


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def require_reports(snapshot, expected, phase):
    reports = snapshot["notifications"]
    require(snapshot["connected"] and snapshot["resolved"], f"{phase}: 连接或服务丢失")
    require(all(item["neutral"] for item in reports), f"{phase}: 观测到非中立输入，请勿操作物理键鼠")
    actual = {(item["report"], item["length"]) for item in reports}
    require(actual == expected, f"{phase}: expected={expected}, actual={actual}")


def calibrate(peer):
    samples = []
    for _ in range(5):
        start = time.monotonic()
        remote = peer.request("clock")["monotonic"]
        end = time.monotonic()
        samples.append({"offset": remote - (start + end) / 2, "uncertainty": (end - start) / 2})
    return min(samples, key=lambda sample: sample["uncertainty"])


def remote_observer(args):
    source = Path(__file__).with_name("ble-observer.py").read_text()
    code = (
        "import sys,tempfile; from pathlib import Path; "
        "root=Path('/tmp/opencode'); root.mkdir(exist_ok=True); "
        "folder=Path(tempfile.mkdtemp(prefix='bt-switch-observer-',dir=root)); "
        "path=folder/'observer.py'; path.write_text(sys.stdin.read()); print(path)"
    )
    ssh = [*shlex.split(args.ssh_command), args.remote]
    copied = subprocess.run([*ssh, f"/usr/bin/python3 -c {shlex.quote(code)}"], input=source,
                            text=True, capture_output=True, check=True, timeout=30)
    path = copied.stdout.strip()
    require(path.startswith("/tmp/opencode/bt-switch-observer-"), "远端观测端路径异常")
    return [*ssh, f"/usr/bin/python3 -u {shlex.quote(path)} --address {shlex.quote(args.address)}"]


class RoutingCheck:
    def __init__(self, serial, peers):
        self.serial = serial
        self.peers = peers
        self.clocks = [calibrate(peer) for peer in peers]

    def reset(self):
        for peer in self.peers:
            peer.request("reset")

    def snapshots(self):
        return [peer.request("status") for peer in self.peers]

    def select(self, slot, release_hosts):
        self.reset()
        sent = self.serial.write(str(slot))
        self.serial.collect(0.5)
        snapshots = self.snapshots()
        latency = []
        for index, snapshot in enumerate(snapshots):
            require_reports(snapshot, EXPECTED if index in release_hosts else set(), f"选择 {slot}，主机 {index + 1} 释放")
            if index in release_hosts:
                first = min(item["time"] for item in snapshot["notifications"])
                estimate = first - self.clocks[index]["offset"] - sent
                error = self.clocks[index]["uncertainty"]
                latency.append({"host": index + 1, "first_release_estimate_ms": estimate * 1000,
                                "clock_uncertainty_ms": error * 1000})
                first_by_report = {}
                for item in snapshot["notifications"]:
                    first_by_report.setdefault(item["report"], item["time"])
                latency[-1]["full_release_estimate_ms"] = (max(first_by_report.values()) - self.clocks[index]["offset"] - sent) * 1000
        return {"slot": slot, "release_latency": latency}

    def probe(self, target):
        self.reset()
        self.serial.exchange("t")
        snapshots = self.snapshots()
        for index, snapshot in enumerate(snapshots):
            expected = EXPECTED if target == index + 1 else set()
            require_reports(snapshot, expected, f"探针目标 {target}，主机 {index + 1}")
        return {"target": target, "counts": [len(state["notifications"]) for state in snapshots]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--remote", required=True)
    parser.add_argument("--ssh-command", default="ssh -o BatchMode=yes -o ConnectTimeout=8")
    parser.add_argument("--address", required=True)
    parser.add_argument("--port", default="/dev/ttyUSB0")
    parser.add_argument("--cycles", type=int, default=5)
    parser.add_argument("--log", required=True)
    args = parser.parse_args()
    require(1 <= args.cycles <= 50, "循环次数必须为 1..50")
    serial = SerialClient(args.port, args.log + ".serial.log")
    peers = []
    original_slot = None
    result = {"passed": False, "switches": [], "probes": []}

    try:
        status = serial.exchange("s")
        result["initial_serial"] = status
        match = re.search(r"slot (\d+)/3 online=2", status)
        require(match is not None, "固件必须已经有两条在线连接")
        original_slot = int(match[1])
        require("3: empty" in status, "第三槽位必须为空，才能检查离线目标")
        for slot in (1, 2):
            require(re.search(rf"slot=Some\({slot}\) encrypted=true", status), f"槽位 {slot} 未加密")

        peers.append(ObserverClient(["/usr/bin/python3", "-u", str(Path(__file__).with_name("ble-observer.py")), "--address", args.address]))
        peers.append(ObserverClient(remote_observer(args)))
        for peer in peers:
            peer.request("subscribe")
        serial.collect(0.5)
        check = RoutingCheck(serial, peers)
        result["clock_calibration"] = check.clocks

        # 先进入槽位 1 并消化初始化释放，不将订阅产生的通知算作探针。
        serial.exchange("1")
        result["probes"].append(check.probe(1))
        for _ in range(args.cycles):
            for slot in (2, 1):
                result["switches"].append(check.select(slot, {0, 1}))
                result["probes"].append(check.probe(slot))

        result["offline_switch"] = check.select(3, {0})
        result["offline_probe"] = check.probe(None)
        status = serial.exchange("s")
        require("slot 3/3 online=2 ready=false" in status, "离线目标断开了连接或错误就绪")
        result["offline_serial"] = status
        result["return_switch"] = check.select(1, {0})
        result["return_probe"] = check.probe(1)
        result["final_serial"] = serial.exchange("s")
        require("online=2" in result["final_serial"] and "encrypted=true" in result["final_serial"], "结束时两条连接未保留")
        result["passed"] = True
    except Exception as error:
        result["error"] = str(error)
    finally:
        if original_slot is not None:
            serial.exchange(str(original_slot))
        for peer in reversed(peers):
            peer.close()
        serial.close()
        Path(args.log).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")

    print(json.dumps(result, ensure_ascii=False, indent=2))
    require(result["passed"], "双机回归未通过；详见日志中的失败阶段")
    print("PASS: 双机在线、切换释放、定向探针与离线目标隔离通过；不代表 USB 输入已验证")


if __name__ == "__main__":
    main()
